//! Which rooms the sidebar shows, and in what sections: the React store's
//! `filterRooms`, `isInSpace` and `groupRooms` (src/store/index.ts), ported
//! so both apps list the same rooms the same way. Pure, so it's tested here.

use horo_core::{
    dto::{RoomMembership, RoomSummary, SpaceSummary},
    matrix::directory::PublicRoom,
};

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
    /// All joined spaces; on Home (`space: None`) with no search/filter, rooms
    /// that already live inside a space stay inside their space, as in Discord.
    pub spaces: &'a [SpaceSummary],
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

/// The joined rooms in a space (not its subspaces).
pub fn in_space<'r>(rooms: &'r [RoomSummary], spaces: &[SpaceSummary], space_id: &str) -> Vec<&'r RoomSummary> {
    let Some(space) = spaces.iter().find(|s| s.id == space_id) else { return Vec::new() };
    rooms.iter().filter(|r| !r.is_space && r.membership == RoomMembership::Joined && is_in_space(r, space)).collect()
}

/// The joined rooms in a space and in no other space you're in: the ones
/// it's safe to offer to leave with it. Leaving one space shouldn't empty
/// another.
pub fn only_in_space<'r>(rooms: &'r [RoomSummary], spaces: &[SpaceSummary], space_id: &str) -> Vec<&'r RoomSummary> {
    in_space(rooms, spaces, space_id).into_iter().filter(|room| !spaces.iter().any(|other| other.id != space_id && is_in_space(room, other))).collect()
}

/// "a, b, c and 4 more": enough to recognise, short enough to read.
pub fn list_names(names: &[&str], shown: usize) -> String {
    if names.len() <= shown {
        return names.join(", ");
    }
    format!("{} and {} more", names[..shown].join(", "), names.len() - shown)
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
            if let Some(space) = view.space {
                if !is_in_space(room, space) || (view.hide_muted && room.is_muted) {
                    return false;
                }
            } else if needle.is_empty()
                && view.filter == Filter::All
                && !room.is_direct
                && room.membership != RoomMembership::Invited
                && view.spaces.iter().any(|s| is_in_space(room, s))
            {
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

/// The room list's stamp, as `formatStamp`: the time today, "yest", the
/// weekday this week, then the date ("21 oct").
pub fn stamp(timestamp_ms: u64, now: chrono::DateTime<chrono::Local>) -> String {
    use chrono::{Local, TimeZone};
    if timestamp_ms == 0 {
        return String::new();
    }
    let Some(then) = Local.timestamp_millis_opt(timestamp_ms as i64).single() else { return String::new() };
    let days = (now.date_naive() - then.date_naive()).num_days();
    match days {
        d if d <= 0 => then.format("%H:%M").to_string(),
        1 => "yest".into(),
        d if d < 7 => then.format("%a").to_string().to_lowercase(),
        _ => then.format("%-d %b").to_string().to_lowercase(),
    }
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
    pub fn title(self, in_space: bool) -> &'static str {
        match (self, in_space) {
            (Section::Invites, _) => "invites",
            (Section::DirectMessages, _) => "direct messages",
            (Section::Rooms, true) => "rooms",
            (Section::Rooms, false) => "group chats",
            (Section::Voice, true) => "voice rooms",
            (Section::Voice, false) => "voice",
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

/// Public or restricted rooms in `space` from its hierarchy that we haven't
/// joined yet, split into text and voice rooms, in the space's own order.
/// Only shown when `filter == Filter::All`: filters like "unread" or "muted"
/// don't apply to rooms we aren't in.
pub fn unjoined_in_space<'a>(
    space: &SpaceSummary,
    hierarchy: &'a [PublicRoom],
    rooms: &[RoomSummary],
    filter: Filter,
    search: &str,
) -> (Vec<&'a PublicRoom>, Vec<&'a PublicRoom>) {
    if filter != Filter::All {
        return (Vec::new(), Vec::new());
    }
    let needle = search.trim().to_lowercase();
    let mut visible: Vec<&PublicRoom> = hierarchy
        .iter()
        .filter(|r| {
            !r.is_space
                && (r.join_rule == "public" || r.join_rule == "restricted")
                && space.children.contains(&r.id)
                && r.members > 0
                && !rooms.iter().any(|x| x.id == r.id && matches!(x.membership, RoomMembership::Joined | RoomMembership::Invited))
                && (needle.is_empty() || r.name.as_deref().unwrap_or_default().to_lowercase().contains(&needle))
        })
        .collect();
    let position = |r: &PublicRoom| space.children.iter().position(|id| *id == r.id).unwrap_or(usize::MAX);
    visible.sort_by(|a, b| {
        position(a)
            .cmp(&position(b))
            .then_with(|| a.name.as_deref().unwrap_or_default().to_lowercase().cmp(&b.name.as_deref().unwrap_or_default().to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    let (voice, text): (Vec<_>, Vec<_>) = visible.into_iter().partition(|r| r.is_voice);
    (text, voice)
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
    fn stamps_read_like_the_webview_builds() {
        use chrono::{Local, TimeZone};
        let now = Local.with_ymd_and_hms(2026, 11, 22, 15, 0, 0).unwrap();
        let at = |y, m, d, h, min| Local.with_ymd_and_hms(y, m, d, h, min, 0).unwrap().timestamp_millis() as u64;
        assert_eq!(stamp(at(2026, 11, 22, 9, 5), now), "09:05");
        assert_eq!(stamp(at(2026, 11, 21, 23, 0), now), "yest");
        assert_eq!(stamp(at(2026, 11, 19, 12, 0), now), "thu");
        assert_eq!(stamp(at(2026, 10, 21, 12, 0), now), "21 oct");
        assert_eq!(stamp(0, now), "");
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

    #[test]
    fn a_spaces_own_rooms_leave_out_ones_another_space_holds() {
        let rooms = vec![room("!a", "a"), room("!b", "b"), room("!c", "c")];
        let spaces = vec![space("!s", &["!a", "!b"]), space("!t", &["!b"])];
        assert_eq!(ids(&in_space(&rooms, &spaces, "!s")), ["!a", "!b"]);
        assert_eq!(ids(&only_in_space(&rooms, &spaces, "!s")), ["!a"]);
        assert!(in_space(&rooms, &spaces, "!nope").is_empty());
        assert_eq!(list_names(&["a", "b"], 3), "a, b");
        assert_eq!(list_names(&["a", "b", "c", "d", "e"], 3), "a, b, c and 2 more");
    }

    #[test]
    fn unjoined_space_rooms_keep_joinable_non_members_in_space_order() {
        let pub_room = |id: &str, name: &str, voice: bool, rule: &str, members: u64| PublicRoom {
            id: id.into(),
            name: Some(name.into()),
            topic: None,
            alias: None,
            avatar_url: None,
            members,
            is_space: false,
            is_voice: voice,
            join_rule: rule.into(),
            via: Vec::new(),
        };
        let s = space("!s", &["!joined", "!b", "!a", "!v", "!priv", "!empty"]);
        let hierarchy = vec![
            pub_room("!joined", "already in", false, "public", 3),
            pub_room("!a", "alpha", false, "restricted", 2),
            pub_room("!b", "beta", false, "public", 5),
            pub_room("!v", "lounge", true, "public", 1),
            pub_room("!priv", "secret", false, "invite", 4),
            pub_room("!empty", "deleted", false, "public", 0),
            pub_room("!stale", "removed from space", false, "public", 2),
        ];
        let joined = vec![room("!joined", "already in")];
        let (text, voice) = unjoined_in_space(&s, &hierarchy, &joined, Filter::All, "");
        assert_eq!(text.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["!b", "!a"]);
        assert_eq!(voice.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["!v"]);
        assert!(unjoined_in_space(&s, &hierarchy, &joined, Filter::Unread, "").0.is_empty());
        let (filtered, _) = unjoined_in_space(&s, &hierarchy, &joined, Filter::All, "alp");
        assert_eq!(filtered.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["!a"]);
    }

    #[test]
    fn home_hides_space_channels_until_searched() {
        let mut dm = room("!dm", "alice");
        dm.is_direct = true;
        let in_server = room("!chan", "general");
        let standalone = room("!group", "friends");
        let spaces = vec![space("!s", &["!chan"])];
        let rooms = vec![dm, in_server, standalone];

        let home = filter(&rooms, &View { spaces: &spaces, ..Default::default() });
        assert_eq!(ids(&home), ["!dm", "!group"].map(String::from));

        let searched = filter(&rooms, &View { spaces: &spaces, search: "gen", ..Default::default() });
        assert_eq!(ids(&searched), ["!chan"].map(String::from));
    }
}
