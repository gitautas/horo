//! horo-core against a real homeserver.
//!
//! These drive the core the way a UI does — sign up, sign in, create rooms,
//! send messages — and check what comes back through the [`EventSink`]. They
//! need a disposable Synapse:
//!
//! ```sh
//! scripts/test-homeserver/start.sh
//! HORO_TEST_HOMESERVER=http://localhost:8008 cargo test -p horo-core --test homeserver
//! ```
//!
//! Without `HORO_TEST_HOMESERVER` every test passes without doing anything, so
//! a plain `cargo test` stays offline. CI sets it, and there a missing server
//! is a failure, not a skip.

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use horo_core::{
    AppState, Event, EventSink, Host,
    dto::{ContentDto, Diff, RegisterAuth, RegisterStep, RoomMembership, RoomSummary, SendOptions, SessionInfo, TimelineItemDto},
    matrix::{auth, rooms, session, timeline},
};
use matrix_sdk::ruma::{OwnedRoomId, RoomId};
use tokio::sync::Notify;

/// How long to wait for the server to reflect a change before giving up.
const PATIENCE: Duration = Duration::from_secs(30);
const PASSWORD: &str = "correct horse battery staple";

fn homeserver() -> Option<String> {
    match std::env::var("HORO_TEST_HOMESERVER") {
        Ok(url) if !url.is_empty() => Some(url),
        _ if std::env::var_os("CI").is_some() => {
            panic!("HORO_TEST_HOMESERVER must be set in CI (see scripts/test-homeserver)")
        }
        _ => {
            eprintln!("HORO_TEST_HOMESERVER not set; skipping");
            None
        }
    }
}

/// An [`EventSink`] that keeps everything it is given, for tests to wait on.
#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<Event>>,
    arrived: Notify,
    opened: Mutex<Vec<String>>,
}

impl EventSink for Recorder {
    fn emit(&self, event: Event) {
        self.events.lock().unwrap().push(event);
        self.arrived.notify_waiters();
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        self.opened.lock().unwrap().push(url.to_owned());
        Ok(())
    }
}

impl Recorder {
    /// Wait until `check` finds what it is looking for in the events so far.
    async fn wait_for<T>(&self, what: &str, mut check: impl FnMut(&[Event]) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            // Register interest before looking, so an event landing between
            // the check and the wait still wakes us.
            let arrived = self.arrived.notified();
            if let Some(found) = check(&self.events.lock().unwrap()) {
                return found;
            }
            if tokio::time::timeout_at(deadline, arrived).await.is_err() {
                panic!("timed out waiting for {what}");
            }
        }
    }
}

/// One signed-in person: their own state, data directory and event log, the
/// way one running copy of the app has them.
struct Person {
    state: AppState,
    recorder: Arc<Recorder>,
    host: Host,
    data_dir: tempfile::TempDir,
    info: SessionInfo,
}

impl Person {
    /// Sign up a brand-new account through the app's own registration flow.
    async fn register(server: &str, name: &str) -> Person {
        session::use_file_storage();

        let recorder = Arc::new(Recorder::default());
        let host: Host = recorder.clone();
        let state = AppState::default();
        let data_dir = tempfile::tempdir().unwrap();
        // Unique per run, since the server outlives the test process.
        let username = format!("{name}-{}", unique());

        // The first request asks which flows the server accepts; this one
        // offers `m.login.dummy`, which the app answers on its own.
        let step = auth::register(
            host.clone(), &state, data_dir.path(), server, &username, PASSWORD, None, None,
        )
        .await
        .expect("registration should start");
        let RegisterStep::NeedsAuth { challenge } = step else {
            panic!("expected the server to ask for auth first");
        };
        let session = Some(challenge.session.clone().expect("a UIA session"));

        let step = auth::register(
            host.clone(),
            &state,
            data_dir.path(),
            server,
            &username,
            PASSWORD,
            session,
            Some(RegisterAuth::Dummy),
        )
        .await
        .expect("registration should finish");
        let RegisterStep::Done { session: info } = step else {
            panic!("expected registration to sign in");
        };

        Person { state, recorder, host, data_dir, info }
    }

    fn dir(&self) -> &Path {
        self.data_dir.path()
    }

    async fn core(&self) -> Arc<horo_core::MatrixCore> {
        self.state.core().await.expect("signed in")
    }

    async fn create_room(&self, name: &str) -> OwnedRoomId {
        let new = serde_json::from_value(serde_json::json!({ "name": name })).unwrap();
        let created = rooms::create(&*self.core().await, new).await.expect("room created");
        RoomId::parse(&created.room_id).unwrap()
    }

    async fn send(&self, room_id: &RoomId, body: &str) {
        let options: SendOptions = serde_json::from_value(serde_json::json!({ "body": body })).unwrap();
        timeline::send(&*self.core().await, room_id, options).await.expect("message sent");
    }

    /// Wait for the room list to hold `room_id`, and return its summary.
    async fn wait_for_room(&self, room_id: &RoomId, what: &str, want: impl Fn(&RoomSummary) -> bool) -> RoomSummary {
        let id = room_id.as_str().to_owned();
        self.recorder
            .wait_for(what, |events| {
                // Replay every batch, as the frontend does, then look.
                let mut list: Vec<RoomSummary> = Vec::new();
                for event in events {
                    if let Event::Rooms(update) = event {
                        for diff in &update.diffs {
                            apply(&mut list, diff);
                        }
                    }
                }
                list.into_iter().find(|room| room.id == id && want(room))
            })
            .await
    }

    /// Wait for a text message with this body in an open timeline.
    async fn wait_for_message(&self, room_id: &RoomId, body: &str) -> TimelineItemDto {
        let key = room_id.as_str().to_owned();
        self.recorder
            .wait_for(&format!("message {body:?}"), |events| {
                events.iter().find_map(|event| match event {
                    Event::Timeline(update) if update.room_id == key => {
                        update.diffs.iter().flat_map(diff_values).find(|item| text_of(item) == Some(body)).cloned()
                    }
                    _ => None,
                })
            })
            .await
    }
}

fn unique() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("{nanos:x}")
}

fn apply<T: Clone>(list: &mut Vec<T>, diff: &Diff<T>) {
    match diff {
        Diff::Append { values } => list.extend(values.iter().cloned()),
        Diff::Clear => list.clear(),
        Diff::PushFront { value } => list.insert(0, value.clone()),
        Diff::PushBack { value } => list.push(value.clone()),
        Diff::PopFront => {
            if !list.is_empty() {
                list.remove(0);
            }
        }
        Diff::PopBack => {
            list.pop();
        }
        Diff::Insert { index, value } => list.insert((*index).min(list.len()), value.clone()),
        Diff::Set { index, value } => {
            if let Some(slot) = list.get_mut(*index) {
                *slot = value.clone();
            }
        }
        Diff::Remove { index } => {
            if *index < list.len() {
                list.remove(*index);
            }
        }
        Diff::Truncate { length } => list.truncate(*length),
        Diff::Reset { values } => *list = values.clone(),
    }
}

/// Every item a diff carries, whatever its position.
fn diff_values<T>(diff: &Diff<T>) -> Box<dyn Iterator<Item = &T> + '_> {
    match diff {
        Diff::Append { values } | Diff::Reset { values } => Box::new(values.iter()),
        Diff::PushFront { value }
        | Diff::PushBack { value }
        | Diff::Insert { value, .. }
        | Diff::Set { value, .. } => Box::new(std::iter::once(value)),
        _ => Box::new(std::iter::empty()),
    }
}

fn text_of(item: &TimelineItemDto) -> Option<&str> {
    match &item.event.as_ref()?.content {
        ContentDto::Text { body, .. } => Some(body),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn registering_signs_in_and_survives_a_restart() {
    let Some(server) = homeserver() else { return };
    let alice = Person::register(&server, "alice").await;

    assert!(alice.info.user_id.starts_with("@alice-"), "{}", alice.info.user_id);
    assert!(alice.info.insecure_storage, "tests must keep secrets out of the keychain");

    // The sync service starts with the session and says so.
    alice
        .recorder
        .wait_for("sync to run", |events| {
            events.iter().any(|e| {
                matches!(e, Event::SyncStatus(s) if s.state == horo_core::dto::SyncState::Running)
            })
            .then_some(())
        })
        .await;

    // Quit, as closing the window does, then start again from disk alone.
    alice.state.take().await.expect("a live session").shutdown().await;

    let relaunched = AppState::default();
    let restored = auth::restore(alice.host.clone(), &relaunched, alice.dir())
        .await
        .expect("restore should succeed")
        .expect("a saved session to restore");
    assert_eq!(restored.user_id, alice.info.user_id);
    assert_eq!(restored.device_id, alice.info.device_id);

    // Restoring twice must hand back the live session, not build a second one
    // on the same store.
    let again = auth::restore(alice.host.clone(), &relaunched, alice.dir()).await.unwrap().unwrap();
    assert_eq!(again.device_id, restored.device_id);

    auth::logout(&relaunched, alice.dir(), true).await.expect("logout");
    assert!(
        auth::restore(alice.host.clone(), &AppState::default(), alice.dir()).await.unwrap().is_none(),
        "nothing should be left to restore after signing out"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn password_login_reaches_the_same_account() {
    let Some(server) = homeserver() else { return };
    let alice = Person::register(&server, "login").await;
    let localpart = alice.info.user_id.trim_start_matches('@').split(':').next().unwrap().to_owned();

    let other_dir = tempfile::tempdir().unwrap();
    let state = AppState::default();
    let host: Host = Arc::new(Recorder::default());

    let wrong = auth::login_password(host.clone(), &state, other_dir.path(), &server, &localpart, "nope").await;
    match wrong {
        Err(horo_core::Error::Auth(message)) => assert_eq!(message, "wrong username or password"),
        other => panic!("expected an auth error, got {other:?}"),
    }

    let info = auth::login_password(host, &state, other_dir.path(), &server, &localpart, PASSWORD)
        .await
        .expect("login");
    assert_eq!(info.user_id, alice.info.user_id);
    assert_ne!(info.device_id, alice.info.device_id, "a second login is a second device");

    if let Some(core) = state.take().await {
        core.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_room_appears_in_the_room_list() {
    let Some(server) = homeserver() else { return };
    let alice = Person::register(&server, "rooms").await;

    let room_id = alice.create_room("tea party").await;
    let summary = alice.wait_for_room(&room_id, "the new room", |r| r.name == "tea party").await;
    assert_eq!(summary.membership, RoomMembership::Joined);
    assert!(!summary.is_space);

    // A snapshot taken now must agree with the stream that produced it.
    let core = alice.core().await;
    let mirror = core.rooms.lock().await;
    assert!(mirror.rooms.iter().any(|r| r.id == room_id.as_str()));
    assert!(mirror.seq > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn messages_reach_the_other_person() {
    let Some(server) = homeserver() else { return };
    let alice = Person::register(&server, "sender").await;
    let bob = Person::register(&server, "receiver").await;

    let room_id = alice.create_room("hello").await;
    rooms::invite(&*alice.core().await, &room_id, &bob.info.user_id).await.expect("invite");

    bob.wait_for_room(&room_id, "the invite", |r| r.membership == RoomMembership::Invited).await;
    rooms::join(&*bob.core().await, room_id.as_str(), &[]).await.expect("join");
    bob.wait_for_room(&room_id, "the join", |r| r.membership == RoomMembership::Joined).await;

    // Both open the room, as selecting it in the sidebar does.
    timeline::open(&alice.host, &alice.core().await, &room_id, None).await.expect("alice's timeline");
    timeline::open(&bob.host, &bob.core().await, &room_id, None).await.expect("bob's timeline");

    alice.send(&room_id, "hi bob~").await;

    let mine = alice.wait_for_message(&room_id, "hi bob~").await;
    assert!(mine.event.as_ref().unwrap().is_own);

    let theirs = bob.wait_for_message(&room_id, "hi bob~").await;
    let event = theirs.event.as_ref().unwrap();
    assert!(!event.is_own);
    assert_eq!(event.sender, alice.info.user_id);
}
