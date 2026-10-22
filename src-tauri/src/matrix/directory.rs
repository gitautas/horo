//! Finding rooms you aren't in yet: the public directory, and a look inside a
//! space before joining it.

use matrix_sdk::ruma::{
    OwnedServerName, RoomId, ServerName, UInt,
    api::client::{directory::get_public_rooms_filtered, space::get_hierarchy},
    directory::{Filter, PublicRoomsChunk, RoomTypeFilter},
    room::{JoinRuleKind, RoomSummary as HierarchySummary, RoomType},
};
use ts_rs::TS;

use crate::{error::Result, matrix::MatrixCore};

/// How many directory entries to ask for at once.
const DIRECTORY_PAGE_SIZE: u32 = 30;

/// How many rooms of a space to show before joining it. The hierarchy pages
/// too, but a preview that needs a "load more" has stopped being a preview.
const PREVIEW_LIMIT: u32 = 50;

/// A room or space as seen from outside: what the directory and a space's
/// hierarchy can tell you without joining.
#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PublicRoom {
    pub id: String,
    pub name: Option<String>,
    pub topic: Option<String>,
    pub alias: Option<String>,
    pub avatar_url: Option<String>,
    pub members: u64,
    pub is_space: bool,
    /// A voice room — see `rooms::is_call_room_type`.
    pub is_voice: bool,
    /// `public` | `knock` | `invite` | `restricted` | … — the spec's spelling.
    /// Only `public` can be walked into; the rest are shown so a preview
    /// doesn't silently hide half a space.
    pub join_rule: String,
    /// Servers to try when joining. The directory's own server for a directory
    /// entry, the space's `via` for a child — a room on someone else's
    /// homeserver can't be joined by bare ID from ours without them.
    pub via: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryPage {
    pub rooms: Vec<PublicRoom>,
    /// Pass back as `since` for the next page; `None` means that was the last.
    pub next_batch: Option<String>,
    /// The server's guess. Some servers don't send one.
    pub total_estimate: Option<u64>,
}

/// Public spaces listed in a server's directory.
///
/// `server` is whose directory to read — ours when `None`. Asking another
/// server goes through ours over federation, which some servers refuse; the
/// error is passed through as-is because it's usually the explanation.
pub async fn public_spaces(
    core: &MatrixCore,
    server: Option<String>,
    search: Option<String>,
    since: Option<String>,
) -> Result<DirectoryPage> {
    let server = server
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(ServerName::parse)
        .transpose()?;

    let mut filter = Filter::new();
    filter.generic_search_term =
        search.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    filter.room_types = vec![RoomTypeFilter::Space];

    let mut request = get_public_rooms_filtered::v3::Request::new();
    request.server = server.clone();
    request.limit = Some(UInt::from(DIRECTORY_PAGE_SIZE));
    request.since = since;
    request.filter = filter;

    let response = core.client.public_rooms_filtered(request).await?;

    // An entry from our own directory needs no `via`; one from elsewhere needs
    // the server we found it on.
    let via: Vec<String> = server.iter().map(|s| s.to_string()).collect();

    Ok(DirectoryPage {
        rooms: response
            .chunk
            .into_iter()
            // The filter is a request, not a guarantee: older servers ignore
            // `room_types` and hand back everything.
            .filter(|chunk| chunk.room_type == Some(RoomType::Space))
            .map(|chunk| from_directory(chunk, &via))
            .collect(),
        next_batch: response.next_batch,
        total_estimate: response.total_room_count_estimate.map(u64::from),
    })
}

/// The rooms directly inside a space, for deciding whether to join it.
///
/// Works on spaces we aren't in as long as they're public or peekable: our
/// server fetches the hierarchy over federation, using `via` to find it.
pub async fn space_rooms(core: &MatrixCore, space_id: &RoomId) -> Result<Vec<PublicRoom>> {
    let mut request = get_hierarchy::v1::Request::new(space_id.to_owned());
    request.max_depth = Some(UInt::from(1u32));
    request.limit = Some(UInt::from(PREVIEW_LIMIT));

    let response = core.client.send(request).await?;

    // The children's `via` live on the root's `m.space.child` events, not on
    // the children themselves.
    let mut via_for = std::collections::HashMap::new();
    for chunk in &response.rooms {
        if chunk.summary.room_id != space_id {
            continue;
        }
        for raw in &chunk.children_state {
            if let Ok(event) = raw.deserialize() {
                let via: Vec<String> = event.content.via.iter().map(|s| s.to_string()).collect();
                via_for.insert(event.state_key, via);
            }
        }
    }

    Ok(response
        .rooms
        .into_iter()
        // The first entry is the space itself.
        .filter(|chunk| chunk.summary.room_id != space_id)
        .map(|chunk| {
            let via = via_for.get(&chunk.summary.room_id).cloned().unwrap_or_default();
            from_hierarchy(chunk.summary, via)
        })
        .collect())
}

fn from_directory(chunk: PublicRoomsChunk, via: &[String]) -> PublicRoom {
    PublicRoom {
        id: chunk.room_id.to_string(),
        name: chunk.name.filter(|n| !n.trim().is_empty()),
        topic: chunk.topic.filter(|t| !t.trim().is_empty()),
        alias: chunk.canonical_alias.map(|a| a.to_string()),
        avatar_url: chunk.avatar_url.map(|u| u.to_string()),
        members: u64::from(chunk.num_joined_members),
        is_space: chunk.room_type == Some(RoomType::Space),
        is_voice: is_voice(chunk.room_type.as_ref()),
        join_rule: join_rule_str(&chunk.join_rule),
        via: via.to_vec(),
    }
}

fn from_hierarchy(summary: HierarchySummary, via: Vec<String>) -> PublicRoom {
    PublicRoom {
        id: summary.room_id.to_string(),
        name: summary.name.filter(|n| !n.trim().is_empty()),
        topic: summary.topic.filter(|t| !t.trim().is_empty()),
        alias: summary.canonical_alias.map(|a| a.to_string()),
        avatar_url: summary.avatar_url.map(|u| u.to_string()),
        members: u64::from(summary.num_joined_members),
        is_space: summary.room_type == Some(RoomType::Space),
        is_voice: is_voice(summary.room_type.as_ref()),
        join_rule: join_rule_str(&summary.join_rule.kind()),
        via,
    }
}

fn is_voice(room_type: Option<&RoomType>) -> bool {
    crate::matrix::rooms::is_call_room_type(room_type.map(|t| t.as_str()))
}

fn join_rule_str(kind: &JoinRuleKind) -> String {
    kind.as_str().to_owned()
}

/// Parse `via` servers, dropping any that don't parse rather than failing the
/// join: they're hints, and the rest may well be enough.
pub fn parse_via(via: &[String]) -> Vec<OwnedServerName> {
    via.iter().filter_map(|s| ServerName::parse(s).ok()).collect()
}

/// Someone the user directory knows, for suggesting as an invite is typed.
#[derive(Debug, Clone, serde::Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct UserSuggestion {
    pub user_id: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// How many people to suggest: enough to find someone, few enough to scan.
const USER_SEARCH_LIMIT: u64 = 8;

/// Search the homeserver's user directory by name or ID.
///
/// Synapse answers with people who share a room with us, plus anyone in a
/// public room — not everyone on the server, so someone new can still be
/// invited by typing their full ID.
pub async fn search_users(core: &MatrixCore, term: &str) -> Result<Vec<UserSuggestion>> {
    let term = term.trim();
    if term.is_empty() {
        return Ok(Vec::new());
    }
    let response = core.client.search_users(term, USER_SEARCH_LIMIT).await?;
    Ok(response
        .results
        .into_iter()
        .map(|user| UserSuggestion {
            user_id: user.user_id.to_string(),
            display_name: user.display_name.filter(|n| !n.trim().is_empty()),
            avatar_url: user.avatar_url.map(|u| u.to_string()),
        })
        .collect())
}
