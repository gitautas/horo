//! Discord-style custom roles stored as a Matrix room/space state event
//! (`im.horo.roles`, state key `""`).
//!
//! Because Matrix has no native named-role spec, roles live in custom room
//! state so all `horo` clients see role colors, hoisted member groups, and
//! `@RoleName` autocomplete. When sending a message with a `@RoleName`
//! mention, the role's members are expanded into standard `m.mentions.user_ids`
//! so every Matrix client and push server notifies them.

use matrix_sdk::{
    deserialized_responses::RawAnySyncOrStrippedState,
    ruma::{RoomId, events::StateEventType},
};
use serde::{Deserialize, Serialize};

use crate::{error::Result, matrix::MatrixCore};

pub const EVENT_TYPE: &str = "im.horo.roles";

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleDto {
    pub id: String,
    pub name: String,
    /// 0xRRGGBB accent color for member names, role pills, and headers.
    pub color: u32,
    #[serde(default = "default_true")]
    pub mentionable: bool,
    /// Display role members in their own group in the member sidebar.
    #[serde(default = "default_true")]
    pub hoist: bool,
    #[serde(default)]
    pub members: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RolesStateContent {
    #[serde(default)]
    roles: Vec<RoleDto>,
}

/// Turn a display name into a clean stable role id slug.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() { "role".to_owned() } else { trimmed.to_owned() }
}

/// Read the custom roles defined on `room_id` (a space or a room).
pub async fn list(core: &MatrixCore, room_id: &RoomId) -> Result<Vec<RoleDto>> {
    let room = core.room(&room_id.to_owned())?;
    let event_type = StateEventType::from(EVENT_TYPE);
    let Some(event) = room.get_state_event(event_type, "").await? else {
        return Ok(Vec::new());
    };
    let value = match &event {
        RawAnySyncOrStrippedState::Sync(raw) => raw.deserialize_as_unchecked::<serde_json::Value>(),
        RawAnySyncOrStrippedState::Stripped(raw) => raw.deserialize_as_unchecked::<serde_json::Value>(),
    };
    let Ok(value) = value else { return Ok(Vec::new()) };
    let content = value.get("content").cloned().unwrap_or_default();
    let parsed: RolesStateContent = serde_json::from_value(content).unwrap_or_default();
    Ok(parsed.roles)
}

/// Replace the custom roles state on `room_id`.
pub async fn save(core: &MatrixCore, room_id: &RoomId, roles: &[RoleDto]) -> Result<()> {
    let room = core.room(&room_id.to_owned())?;
    let body = serde_json::to_value(RolesStateContent { roles: roles.to_vec() })
        .map_err(|e| crate::error::Error::Other(e.to_string()))?;
    room.send_state_event_raw(EVENT_TYPE, "", body).await?;
    Ok(())
}

/// Whether this account has permission to edit `im.horo.roles` in `room_id`.
pub async fn can_manage(core: &MatrixCore, room_id: &RoomId) -> Result<bool> {
    let room = core.room(&room_id.to_owned())?;
    let own = core.own_user_id()?;
    let Ok(pl) = room.power_levels().await else {
        return Ok(false);
    };
    Ok(pl.user_can_send_state(&own, StateEventType::from(EVENT_TYPE)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_role_names_cleanly() {
        assert_eq!(slug("Server Admins"), "server-admins");
        assert_eq!(slug("  VIP!! "), "vip");
        assert_eq!(slug("✨"), "role");
    }

    #[test]
    fn roles_content_round_trips() {
        let state = RolesStateContent {
            roles: vec![RoleDto {
                id: "gamers".into(),
                name: "Gamers".into(),
                color: 0x5865F2,
                mentionable: true,
                hoist: true,
                members: vec!["@alice:example.org".into()],
            }],
        };
        let json = serde_json::to_value(&state).unwrap();
        let back: RolesStateContent = serde_json::from_value(json).unwrap();
        assert_eq!(back.roles, state.roles);
    }
}
