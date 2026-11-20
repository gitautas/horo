//! The app's state on gpui's side: who's signed in, the room list, the
//! spaces, the connection. One [`Store`] entity; views observe it.
//!
//! It is fed from two directions: answers to calls it makes through the
//! [`Backend`], and horo-core's [`Event`]s, which arrive on a channel and are
//! folded in on gpui's executor.

use std::time::{Duration, Instant};

use gpui::{Context, SharedString};
use horo_core::{
    Event,
    dto::{Diff, RoomSummary, SessionInfo, SpaceSummary, SyncState},
    matrix::{auth, rooms},
};

use crate::backend::Backend;

/// Where sign-in stands.
#[derive(Debug, Clone)]
pub enum Phase {
    /// Looking for a saved session.
    Restoring,
    SignedOut,
    SigningIn,
    SignedIn(SessionInfo),
}

pub struct Store {
    pub phase: Phase,
    /// The last sign-in failure, for the login screen.
    pub error: Option<SharedString>,
    pub rooms: Vec<RoomSummary>,
    /// The last diff batch folded into `rooms`; `None` until the snapshot.
    rooms_seq: Option<u64>,
    /// Batches that arrived before the snapshot.
    early: Vec<horo_core::dto::RoomsUpdate>,
    pub spaces: Vec<SpaceSummary>,
    spaces_fetched: Option<Instant>,
    /// A refresh is waiting out the throttle.
    spaces_pending: bool,
    pub sync: Option<SyncState>,
    /// The space open in the rail; `None` is home.
    pub space: Option<String>,
    pub room: Option<String>,
}

/// Spaces are recomputed from the room list, at most this often.
const SPACES_EVERY: Duration = Duration::from_secs(2);

impl Store {
    pub fn new(cx: &mut Context<Self>) -> Store {
        let mut store = Store {
            phase: Phase::Restoring,
            error: None,
            rooms: Vec::new(),
            rooms_seq: None,
            early: Vec::new(),
            spaces: Vec::new(),
            spaces_fetched: None,
            spaces_pending: false,
            sync: None,
            space: None,
            room: None,
        };
        store.pump_events(cx);
        store.restore(cx);
        store
    }

    /// Fold the core's events in as they come.
    fn pump_events(&mut self, cx: &mut Context<Self>) {
        let Some(mut events) = Backend::global(cx).take_events() else { return };
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this.update(cx, |store, cx| store.apply(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Rooms(update) => {
                match self.rooms_seq {
                    None => self.early.push(update),
                    Some(seq) if update.seq <= seq => {}
                    Some(_) => {
                        self.rooms_seq = Some(update.seq);
                        apply_diffs(&mut self.rooms, update.diffs);
                    }
                }
                self.refresh_spaces(cx);
            }
            Event::SyncStatus(status) => self.sync = Some(status.state),
            _ => return,
        }
        cx.notify();
    }

    fn restore(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let task = backend.run({
            let (host, state, dir) = (backend.host.clone(), backend.state.clone(), backend.data_dir.clone());
            async move { auth::restore(host, &state, &dir).await }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |store, cx| match outcome {
                Ok(Ok(Some(info))) => store.signed_in(info, cx),
                Ok(Ok(None)) => store.set_phase(Phase::SignedOut, cx),
                Ok(Err(e)) => store.failed(e.to_string(), cx),
                Err(e) => store.failed(e.to_string(), cx),
            })
        })
        .detach();
    }

    pub fn sign_in(&mut self, server: String, username: String, password: String, cx: &mut Context<Self>) {
        self.error = None;
        self.set_phase(Phase::SigningIn, cx);
        let backend = Backend::global(cx);
        let task = backend.run({
            let (host, state, dir) = (backend.host.clone(), backend.state.clone(), backend.data_dir.clone());
            async move { auth::login_password(host, &state, &dir, &server, &username, &password).await }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |store, cx| match outcome {
                Ok(Ok(info)) => store.signed_in(info, cx),
                Ok(Err(e)) => store.failed(e.to_string(), cx),
                Err(e) => store.failed(e.to_string(), cx),
            })
        })
        .detach();
    }

    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let task = backend.run({
            let (state, dir) = (backend.state.clone(), backend.data_dir.clone());
            async move { auth::logout(&state, &dir, false).await }
        });
        self.rooms.clear();
        self.rooms_seq = None;
        self.spaces.clear();
        self.room = None;
        self.space = None;
        self.set_phase(Phase::SignedOut, cx);
        cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = task.await {
                let _ = this.update(cx, |store, cx| store.failed(format!("signing out: {e}"), cx));
            }
        })
        .detach();
    }

    fn failed(&mut self, error: String, cx: &mut Context<Self>) {
        tracing::warn!("{error}");
        self.error = Some(error.into());
        if !matches!(self.phase, Phase::SignedIn(_)) {
            self.phase = Phase::SignedOut;
        }
        cx.notify();
    }

    fn set_phase(&mut self, phase: Phase, cx: &mut Context<Self>) {
        self.phase = phase;
        cx.notify();
    }

    /// Signed in: take the room list as it stands, then follow its diffs.
    fn signed_in(&mut self, info: SessionInfo, cx: &mut Context<Self>) {
        self.error = None;
        self.set_phase(Phase::SignedIn(info), cx);
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                let mirror = core.rooms.lock().await;
                Ok::<_, horo_core::Error>((mirror.seq, mirror.rooms.clone()))
            }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |store, cx| match outcome {
                Ok(Ok((seq, rooms))) => {
                    store.rooms = rooms;
                    store.rooms_seq = Some(seq);
                    for update in std::mem::take(&mut store.early) {
                        if update.seq > seq {
                            store.rooms_seq = Some(update.seq);
                            apply_diffs(&mut store.rooms, update.diffs);
                        }
                    }
                    store.spaces_fetched = None;
                    store.refresh_spaces(cx);
                    cx.notify();
                }
                Ok(Err(e)) => store.failed(e.to_string(), cx),
                Err(e) => store.failed(e.to_string(), cx),
            })
        })
        .detach();
    }

    /// Spaces come from the client, not the room list's diffs; ask again
    /// when the list moves, no more often than [`SPACES_EVERY`].
    fn refresh_spaces(&mut self, cx: &mut Context<Self>) {
        if let Some(fetched) = self.spaces_fetched
            && fetched.elapsed() < SPACES_EVERY
        {
            // Throttled: one more fetch once the interval is up, so the last
            // change in a burst (the initial sync's, say) is never missed.
            if !self.spaces_pending {
                self.spaces_pending = true;
                let wait = SPACES_EVERY - fetched.elapsed();
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update(cx, |store, cx| {
                        store.spaces_pending = false;
                        store.refresh_spaces(cx);
                    });
                })
                .detach();
            }
            return;
        }
        self.spaces_fetched = Some(Instant::now());
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { rooms::spaces(&*state.core().await?).await }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(spaces)) = task.await {
                let _ = this.update(cx, |store, cx| {
                    store.spaces = spaces;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn open_space(&mut self, space: Option<String>, cx: &mut Context<Self>) {
        self.space = space;
        cx.notify();
    }

    pub fn open_room(&mut self, room: String, cx: &mut Context<Self>) {
        self.room = Some(room);
        cx.notify();
    }

    pub fn current_space(&self) -> Option<&SpaceSummary> {
        let id = self.space.as_ref()?;
        self.spaces.iter().find(|s| s.id == *id)
    }
}

/// `eyeball_im`'s diffs, applied to a plain list.
pub fn apply_diffs<T: Clone>(list: &mut Vec<T>, diffs: Vec<Diff<T>>) {
    for diff in diffs {
        match diff {
            Diff::Append { values } => list.extend(values),
            Diff::Clear => list.clear(),
            Diff::PushFront { value } => list.insert(0, value),
            Diff::PushBack { value } => list.push(value),
            Diff::PopFront => {
                if !list.is_empty() {
                    list.remove(0);
                }
            }
            Diff::PopBack => {
                list.pop();
            }
            Diff::Insert { index, value } => list.insert(index.min(list.len()), value),
            Diff::Set { index, value } => {
                if let Some(slot) = list.get_mut(index) {
                    *slot = value;
                }
            }
            Diff::Remove { index } => {
                if index < list.len() {
                    list.remove(index);
                }
            }
            Diff::Truncate { length } => list.truncate(length),
            Diff::Reset { values } => *list = values,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_apply_in_order_and_out_of_range_ones_are_ignored() {
        let mut list = vec![1, 2, 3];
        apply_diffs(
            &mut list,
            vec![
                Diff::PushFront { value: 0 },
                Diff::Set { index: 3, value: 30 },
                Diff::Remove { index: 1 },
                Diff::Insert { index: 99, value: 9 },
                Diff::Set { index: 99, value: 0 },
                Diff::Remove { index: 99 },
                Diff::PopBack,
                Diff::Append { values: vec![7, 8] },
            ],
        );
        assert_eq!(list, vec![0, 2, 30, 7, 8]);
        apply_diffs(&mut list, vec![Diff::Truncate { length: 2 }, Diff::PopFront]);
        assert_eq!(list, vec![2]);
        apply_diffs(&mut list, vec![Diff::Reset { values: vec![5] }, Diff::PushBack { value: 6 }]);
        assert_eq!(list, vec![5, 6]);
    }
}
