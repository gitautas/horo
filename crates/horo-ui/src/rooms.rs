//! Which rooms the sidebar shows, and in what sections: the React store's
//! `filterRooms`, `isInSpace` and `groupRooms` (src/store/index.ts), ported
//! so both apps list the same rooms the same way. Pure, so it's tested here.

use horo_core::dto::{RoomMembership, RoomSummary, SpaceSummary};

/// The list's quick filter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Unread,
    Dms,
    Muted,
}

/// What narrows the list: the open space, the filter, the search box.
#[derive(Debug, Clone, Default)]
pub struct View<'a> {
    pub space: Option<&'a SpaceSummary>,
    pub filter: Filter,
    pub search: &'a str,
    /// Leave muted rooms out; only applies inside a space, as in Discord.
    pub hide_muted: bool,
}

/// Whether a room belongs to a space. The space's children list is the
/// primary source; a room's own `m.space.parent` is the fallback, since most
/// rooms never set it.
pub fn is_in_space(room: &RoomSummary, space: &SpaceSummary) -> bool {
    space.children.contains(&room.id) || room.parent_spaces.contains(&space.id)
}

/// The rooms to list, newest activity first. Spaces are never listed (they
/// are the rail) unless it's an invite to one outside any space.
pub fn filter<'r>(rooms: &'r [RoomSummary], view: &View) -> Vec<&'r RoomSummary> {
    let needle = view.search.trim().to_lowercase();
    let mut visible: Vec<&RoomSummary> = rooms
        .iter()
        .filter(|room| {
            if room.is_space && (room.membership != RoomMembership::Invited || view.space.is_some()) {
                return false;
            }
            if room.is_utility || matches!(room.membership, RoomMembership::Left | RoomMembership::Banned) {
                return false;
            }
            if let Some(space) = view.space
                && (!is_in_space(room, space) || (view.hide_muted && room.is_muted)) {
                    return false;
                }
            let keep = match view.filter {
                Filter::All => true,
                Filter::Unread => room.has_unread,
                Filter::Dms => room.is_direct,
                Filter::Muted => room.is_muted,
            };
            if !keep {
                return false;
            }
            if !needle.is_empty() {
                let haystack = format!(
                    "{} {} {}",
                    room.name,
                    room.canonical_alias.as_deref().unwrap_or_default(),
                    room.topic.as_deref().unwrap_or_default()
                );
                if !haystack.to_lowercase().contains(&needle) {
                    return false;
                }
            }
            true
        })
        .collect();
    let when = |r: &RoomSummary| r.latest.as_ref().map_or(r.recency, |l| l.timestamp);
    visible.sort_by(|a, b| when(b).cmp(&when(a)).then_with(|| a.name.cmp(&b.name)));
    visible
}

/// A section of the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Invites,
    DirectMessages,
    Rooms,
    Voice,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Invites => "invites",
            Section::DirectMessages => "direct messages",
            Section::Rooms => "rooms",
            Section::Voice => "voice",
        }
    }
}

/// Invites first (a question waiting on you), then people, then rooms and
/// voice rooms. DMs keep `filter`'s newest-first order; rooms are places and
/// stay put: in the space's own order (`order`), then by name.
pub fn group<'r>(rooms: Vec<&'r RoomSummary>, order: &[String]) -> Vec<(Section, Vec<&'r RoomSummary>)> {
    let (invites, joined): (Vec<_>, Vec<_>) = rooms.into_iter().partition(|r| r.membership == RoomMembership::Invited);
    let (dms, mut channels): (Vec<_>, Vec<_>) = joined.into_iter().partition(|r| r.is_direct);
    let position = |r: &RoomSummary| order.iter().position(|id| *id == r.id).unwrap_or(usize::MAX);
    channels.sort_by(|a, b| position(a).cmp(&position(b)).then_with(|| a.name.cmp(&b.name)));
    let (voice, text): (Vec<_>, Vec<_>) = channels.into_iter().partition(|r| r.is_video_room);

    [(Section::Invites, invites), (Section::DirectMessages, dms), (Section::Rooms, text), (Section::Voice, voice)]
        .into_iter()
        .filter(|(_, rooms)| !rooms.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use horo_core::dto::LatestEvent;

    fn room(id: &str, name: &str) -> RoomSummary {
        RoomSummary {
            id: id.into(),
            name: name.into(),
            topic: None,
            canonical_alias: None,
            avatar_url: None,
            is_direct: false,
            dm_user_id: None,
            is_encrypted: false,
            is_space: false,
            is_utility: false,
            is_muted: false,
            notify_mode: None,
            membership: RoomMembership::Joined,
            notification_count: 0,
            highlight_count: 0,
            has_unread: false,
            is_marked_unread: false,
            member_count: 2,
            latest: None,
            recency: 0,
            parent_spaces: Vec::new(),
            has_active_call: false,
            call_participants: Vec::new(),
            is_video_room: false,
        }
    }

    fn space(id: &str, children: &[&str]) -> SpaceSummary {
        SpaceSummary {
            id: id.into(),
            name: id.into(),
            avatar_url: None,
            children: children.iter().map(|c| c.to_string()).collect(),
            notification_count: 0,
            highlight_count: 0,
        }
    }

    fn ids(rooms: &[&RoomSummary]) -> Vec<String> {
        rooms.iter().map(|r| r.id.clone()).collect()
    }

    #[test]
    fn spaces_utility_rooms_and_left_rooms_are_not_listed() {
        let mut s = room("!s", "a space");
        s.is_space = true;
        let mut pack = room("!p", "stickers");
        pack.is_utility = true;
        let mut left = room("!l", "gone");
        left.membership = RoomMembership::Left;
        let mut invited_space = room("!is", "come in");
        invited_space.is_space = true;
        invited_space.membership = RoomMembership::Invited;
        let rooms = vec![s, pack, left, room("!r", "general"), invited_space];
        let shown = filter(&rooms, &View::default());
        assert_eq!(ids(&shown), ["!is", "!r"].map(String::from));
    }

    #[test]
    fn a_space_shows_its_children_and_rooms_that_name_it_as_parent() {
        let mut parented = room("!b", "b");
        parented.parent_spaces = vec!["!s".into()];
        let rooms = vec![room("!a", "a"), parented, room("!c", "c")];
        let s = space("!s", &["!a"]);
        let shown = filter(&rooms, &View { space: Some(&s), ..Default::default() });
        assert_eq!(ids(&shown), ["!a", "!b"].map(String::from));
    }

    #[test]
    fn newest_first_and_search_matches_name_alias_and_topic() {
        let mut old = room("!old", "old news");
        old.recency = 1;
        let mut new = room("!new", "fresh");
        new.latest = Some(LatestEvent { sender: "@a:x".into(), sender_name: None, body: "hi".into(), timestamp: 5 });
        new.topic = Some("Rust and gpui".into());
        let rooms = vec![old, new];
        assert_eq!(ids(&filter(&rooms, &View::default())), ["!new", "!old"].map(String::from));
        assert_eq!(ids(&filter(&rooms, &View { search: "GPUI", ..Default::default() })), ["!new"].map(String::from));
    }

    #[test]
    fn sections_put_invites_first_and_keep_rooms_in_the_spaces_order() {
        let mut invite = room("!i", "invite");
        invite.membership = RoomMembership::Invited;
        let mut dm = room("!d", "bob");
        dm.is_direct = true;
        let mut voice = room("!v", "lounge");
        voice.is_video_room = true;
        let rooms = [room("!z", "zeta"), room("!a", "alpha"), invite, dm, voice];
        let refs: Vec<&RoomSummary> = rooms.iter().collect();
        let groups = group(refs, &["!z".to_string()]);
        let shape: Vec<(Section, Vec<String>)> = groups.iter().map(|(s, r)| (*s, ids(r))).collect();
        assert_eq!(
            shape,
            vec![
                (Section::Invites, vec!["!i".into()]),
                (Section::DirectMessages, vec!["!d".into()]),
                (Section::Rooms, vec!["!z".into(), "!a".into()]),
                (Section::Voice, vec!["!v".into()]),
            ]
        );
    }
}
