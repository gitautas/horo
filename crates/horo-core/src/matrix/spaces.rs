//! Space Settings backend: native Matrix Space metadata (name, description,
//! icon, join rules, members, power levels, bans) plus `im.horo.space_profile`
//! state (banner preset, traits, and space tag).

use matrix_sdk::{
    RoomMemberships,
    deserialized_responses::RawAnySyncOrStrippedState,
    ruma::{
        Int, OwnedRoomAliasId, RoomId, UserId,
        events::{
            StateEventType,
            room::{
                canonical_alias::RoomCanonicalAliasEventContent,
                join_rules::{JoinRule, RoomJoinRulesEventContent},
                power_levels::{PowerLevelAction, UserPowerLevel},
            },
        },
    },
};
use serde::{Deserialize, Serialize};

use crate::{
    dto::{RoomMemberDto, Verification},
    error::{Error, Result},
    matrix::{MatrixCore, roles},
};

pub const PROFILE_EVENT_TYPE: &str = "im.horo.space_profile";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceTrait {
    pub emoji: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceProfileDto {
    /// Banner gradient preset id (e.g. "pink", "blurple", "coral", ...).
    #[serde(default)]
    pub banner: Option<String>,
    /// Custom uploaded banner image (`mxc://...`). Takes priority over `banner`.
    #[serde(default)]
    pub banner_url: Option<String>,
    /// Short 2-4 char badge tag for the space.
    #[serde(default)]
    pub tag: Option<String>,
    /// Up to 5 short personality/interest traits.
    #[serde(default)]
    pub traits: Vec<SpaceTrait>,
    /// Optional custom invite link base URL (e.g. `https://m.uwu.lt/#/`),
    /// replacing the default `https://matrix.to/#/`.
    #[serde(default)]
    pub link_base: Option<String>,
}

/// Extract the homeserver domain from a Matrix room ID (`!id:domain`) or
/// canonical alias (`#alias:domain`).
pub fn space_server_name(address: &str) -> Option<&str> {
    address.split_once(':').map(|(_, host)| host.trim()).filter(|h| !h.is_empty())
}

/// Build the shareable invite URL for a Space. Uses `link_base` when set;
/// otherwise infers `https://<homeserver>/#/` directly from the Space's
/// alias or room ID (falling back to `https://matrix.to/#/`).
pub fn space_invite_url(link_base: Option<&str>, address: &str) -> String {
    let inferred;
    let base = match link_base.map(str::trim).filter(|s| !s.is_empty()) {
        Some(custom) => custom,
        None => match space_server_name(address) {
            Some(host) => {
                inferred = format!("https://{host}/#/");
                inferred.as_str()
            }
            None => "https://matrix.to/#/",
        },
    };
    if base.ends_with('/') || base.ends_with('#') || base.ends_with('=') {
        format!("{base}{address}")
    } else {
        format!("{base}/{address}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceDetailsDto {
    pub id: String,
    pub name: String,
    pub topic: String,
    pub avatar_url: Option<String>,
    pub canonical_alias: Option<String>,
    pub homeserver_domain: String,
    pub member_count: u64,
    pub join_rule: String,
    pub profile: SpaceProfileDto,
    pub can_edit_name: bool,
    pub can_edit_topic: bool,
    pub can_edit_avatar: bool,
    pub can_edit_profile: bool,
    pub can_edit_access: bool,
    pub can_edit_alias: bool,
    pub can_manage_roles: bool,
    pub can_kick: bool,
    pub can_ban: bool,
    pub can_change_power: bool,
}

/// Read `im.horo.space_profile` from a Space.
pub async fn get_profile(core: &MatrixCore, space_id: &RoomId) -> Result<SpaceProfileDto> {
    let room = core.room(&space_id.to_owned())?;
    let Some(event) = room.get_state_event(StateEventType::from(PROFILE_EVENT_TYPE), "").await? else {
        return Ok(SpaceProfileDto::default());
    };
    let value = match &event {
        RawAnySyncOrStrippedState::Sync(raw) => raw.deserialize_as_unchecked::<serde_json::Value>(),
        RawAnySyncOrStrippedState::Stripped(raw) => raw.deserialize_as_unchecked::<serde_json::Value>(),
    };
    let Ok(value) = value else {
        return Ok(SpaceProfileDto::default());
    };
    let content = value.get("content").cloned().unwrap_or_default();
    Ok(serde_json::from_value(content).unwrap_or_default())
}

/// Write `im.horo.space_profile` to a Space.
pub async fn save_profile(core: &MatrixCore, space_id: &RoomId, profile: &SpaceProfileDto) -> Result<()> {
    let room = core.room(&space_id.to_owned())?;
    let body = serde_json::to_value(profile).map_err(|e| Error::Other(e.to_string()))?;
    room.send_state_event_raw(PROFILE_EVENT_TYPE, "", body).await?;
    Ok(())
}

/// Read full Space details, permissions, and custom profile for Space Settings.
pub async fn details(core: &MatrixCore, space_id: &RoomId) -> Result<SpaceDetailsDto> {
    let room = core.room(&space_id.to_owned())?;
    let own = core.own_user_id()?;
    let name = room.display_name().await.map(|d| d.to_string()).unwrap_or_else(|_| space_id.to_string());
    let topic = room.topic().unwrap_or_default();
    let avatar_url = room.avatar_url().map(|u| u.to_string());
    let canonical_alias = room.canonical_alias().map(|a| a.to_string());
    let member_count = room.joined_members_count();
    let join_rule = match room.join_rule() {
        Some(JoinRule::Public) => "public",
        Some(JoinRule::Knock) | Some(JoinRule::KnockRestricted(_)) => "knock",
        Some(JoinRule::Restricted(_)) => "restricted",
        _ => "invite",
    }
    .to_owned();
    let profile = get_profile(core, space_id).await.unwrap_or_default();

    let (
        can_edit_name,
        can_edit_topic,
        can_edit_avatar,
        can_edit_profile,
        can_edit_access,
        can_edit_alias,
        can_manage_roles,
        can_kick,
        can_ban,
        can_change_power,
    ) = match room.power_levels().await {
        Ok(pl) => (
            pl.user_can_send_state(&own, StateEventType::RoomName),
            pl.user_can_send_state(&own, StateEventType::RoomTopic),
            pl.user_can_send_state(&own, StateEventType::RoomAvatar),
            pl.user_can_send_state(&own, StateEventType::from(PROFILE_EVENT_TYPE)),
            pl.user_can_send_state(&own, StateEventType::RoomJoinRules),
            pl.user_can_send_state(&own, StateEventType::RoomCanonicalAlias),
            pl.user_can_send_state(&own, StateEventType::from(roles::EVENT_TYPE)),
            pl.user_can_do(&own, PowerLevelAction::Kick),
            pl.user_can_do(&own, PowerLevelAction::Ban),
            pl.user_can_send_state(&own, StateEventType::RoomPowerLevels),
        ),
        Err(_) => (false, false, false, false, false, false, false, false, false, false),
    };

    Ok(SpaceDetailsDto {
        id: space_id.to_string(),
        name,
        topic,
        avatar_url,
        canonical_alias,
        homeserver_domain: own.server_name().to_string(),
        member_count,
        join_rule,
        profile,
        can_edit_name,
        can_edit_topic,
        can_edit_avatar,
        can_edit_profile,
        can_edit_access,
        can_edit_alias,
        can_manage_roles,
        can_kick,
        can_ban,
        can_change_power,
    })
}

/// Upload a custom Space banner image to Matrix media and return its `mxc://` URI.
pub async fn upload_banner(core: &MatrixCore, bytes: Vec<u8>, mime_str: &str) -> Result<String> {
    let mime: mime::Mime = mime_str.parse().map_err(|_| Error::Other("unsupported image format".into()))?;
    let res = core.client.media().upload(&mime, bytes, None).await?;
    Ok(res.content_uri.to_string())
}

/// Upload a new Space icon (`m.room.avatar`).
pub async fn upload_avatar(core: &MatrixCore, space_id: &RoomId, bytes: Vec<u8>, mime_str: &str) -> Result<()> {
    let room = core.room(&space_id.to_owned())?;
    let mime: mime::Mime = mime_str.parse().map_err(|_| Error::Other("unsupported image format".into()))?;
    room.upload_avatar(&mime, bytes, None).await?;
    Ok(())
}

/// Remove the Space's icon (`m.room.avatar`).
pub async fn remove_avatar(core: &MatrixCore, space_id: &RoomId) -> Result<()> {
    let room = core.room(&space_id.to_owned())?;
    room.remove_avatar().await?;
    Ok(())
}

/// Set (or create) the Space's canonical alias `#<localpart>:<homeserver>`.
pub async fn set_canonical_alias(core: &MatrixCore, space_id: &RoomId, localpart: &str) -> Result<String> {
    let room = core.room(&space_id.to_owned())?;
    let own = core.own_user_id()?;
    let clean = localpart.trim().trim_start_matches('#');
    if clean.is_empty() {
        let mut content = RoomCanonicalAliasEventContent::new();
        content.alt_aliases = room.alt_aliases();
        room.send_state_event(content).await?;
        return Ok(String::new());
    }
    let full = if clean.contains(':') {
        format!("#{clean}")
    } else {
        format!("#{clean}:{}", own.server_name())
    };
    let alias = OwnedRoomAliasId::try_from(full.as_str())
        .map_err(|e| Error::Other(format!("invalid room alias '{full}': {e}")))?;

    // Register the alias on the homeserver unless it already resolves to this space.
    if let Err(err) = core.client.create_room_alias(&alias, space_id).await {
        let already_ours = core
            .client
            .resolve_room_alias(&alias)
            .await
            .is_ok_and(|res| res.room_id == space_id);
        if !already_ours {
            return Err(Error::Other(err.to_string()));
        }
    }

    let mut content = RoomCanonicalAliasEventContent::new();
    content.alias = Some(alias.clone());
    content.alt_aliases = room.alt_aliases();
    room.send_state_event(content).await?;
    Ok(alias.to_string())
}

/// Set the Space's `m.room.join_rules` ("invite", "knock", or "public").
pub async fn set_join_rule(core: &MatrixCore, space_id: &RoomId, rule: &str) -> Result<()> {
    let room = core.room(&space_id.to_owned())?;
    let join_rule = match rule {
        "public" => JoinRule::Public,
        "knock" => JoinRule::Knock,
        _ => JoinRule::Invite,
    };
    room.send_state_event(RoomJoinRulesEventContent::new(join_rule)).await?;
    Ok(())
}

/// List banned users in a Space.
pub async fn banned_members(core: &MatrixCore, space_id: &RoomId) -> Result<Vec<RoomMemberDto>> {
    let room = core.room(&space_id.to_owned())?;
    let members = room.members(RoomMemberships::BAN).await?;
    Ok(members
        .into_iter()
        .map(|m| RoomMemberDto {
            user_id: m.user_id().to_string(),
            display_name: m.display_name().map(str::to_owned),
            avatar_url: m.avatar_url().map(|u| u.to_string()),
            power_level: match m.power_level() {
                UserPowerLevel::Infinite => i64::MAX,
                UserPowerLevel::Int(v) => v.into(),
                _ => 0,
            },
            membership: "ban".to_owned(),
            is_ignored: m.is_ignored(),
            verification: Verification::Unverified,
        })
        .collect())
}

pub async fn kick_member(core: &MatrixCore, space_id: &RoomId, user_id: &str) -> Result<()> {
    let room = core.room(&space_id.to_owned())?;
    let uid = UserId::parse(user_id)?;
    room.kick_user(&uid, None).await?;
    Ok(())
}

pub async fn ban_member(core: &MatrixCore, space_id: &RoomId, user_id: &str) -> Result<()> {
    let room = core.room(&space_id.to_owned())?;
    let uid = UserId::parse(user_id)?;
    room.ban_user(&uid, None).await?;
    Ok(())
}

pub async fn unban_member(core: &MatrixCore, space_id: &RoomId, user_id: &str) -> Result<()> {
    let room = core.room(&space_id.to_owned())?;
    let uid = UserId::parse(user_id)?;
    room.unban_user(&uid, None).await?;
    Ok(())
}

pub async fn set_member_power(core: &MatrixCore, space_id: &RoomId, user_id: &str, level: i64) -> Result<()> {
    let room = core.room(&space_id.to_owned())?;
    let uid = UserId::parse(user_id)?;
    let int = Int::new(level).ok_or_else(|| Error::Other("power level out of range".into()))?;
    room.update_power_levels(vec![(&uid, int)]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_profile_round_trips() {
        let profile = SpaceProfileDto {
            banner: Some("pink".into()),
            banner_url: Some("mxc://example.org/banner123".into()),
            tag: Some("UWU".into()),
            traits: vec![SpaceTrait { emoji: "🎮".into(), label: "gaming".into() }],
            link_base: Some("https://m.uwu.lt/#/".into()),
        };
        let json = serde_json::to_value(&profile).unwrap();
        let back: SpaceProfileDto = serde_json::from_value(json).unwrap();
        assert_eq!(back, profile);
    }

    #[test]
    fn builds_space_invite_url_with_default_or_custom_domain() {
        assert_eq!(space_invite_url(None, "#uwu:m.uwu.lt"), "https://m.uwu.lt/#/#uwu:m.uwu.lt");
        assert_eq!(space_invite_url(Some(""), "!abc:m.uwu.lt"), "https://m.uwu.lt/#/!abc:m.uwu.lt");
        assert_eq!(space_invite_url(Some("https://matrix.to/#/"), "#uwu:m.uwu.lt"), "https://matrix.to/#/#uwu:m.uwu.lt");
        assert_eq!(space_invite_url(Some("https://uwu.lt/join"), "#uwu:m.uwu.lt"), "https://uwu.lt/join/#uwu:m.uwu.lt");
    }
}
