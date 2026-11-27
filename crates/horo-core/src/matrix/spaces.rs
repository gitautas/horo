//! Space Settings backend: native Matrix Space metadata (name, description,
//! icon, join rules, members, power levels, bans) plus `im.horo.space_profile`
//! state (banner preset, traits, and space tag).

use matrix_sdk::{
    RoomMemberships,
    deserialized_responses::RawAnySyncOrStrippedState,
    ruma::{
        Int, RoomId, UserId,
        events::{
            StateEventType,
            room::join_rules::{JoinRule, RoomJoinRulesEventContent},
            room::power_levels::{PowerLevelAction, UserPowerLevel},
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
    /// Short 2-4 char badge tag for the space.
    #[serde(default)]
    pub tag: Option<String>,
    /// Up to 5 short personality/interest traits.
    #[serde(default)]
    pub traits: Vec<SpaceTrait>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceDetailsDto {
    pub id: String,
    pub name: String,
    pub topic: String,
    pub avatar_url: Option<String>,
    pub canonical_alias: Option<String>,
    pub member_count: u64,
    pub join_rule: String,
    pub profile: SpaceProfileDto,
    pub can_edit_name: bool,
    pub can_edit_topic: bool,
    pub can_edit_avatar: bool,
    pub can_edit_profile: bool,
    pub can_edit_access: bool,
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
            pl.user_can_send_state(&own, StateEventType::from(roles::EVENT_TYPE)),
            pl.user_can_do(&own, PowerLevelAction::Kick),
            pl.user_can_do(&own, PowerLevelAction::Ban),
            pl.user_can_send_state(&own, StateEventType::RoomPowerLevels),
        ),
        Err(_) => (false, false, false, false, false, false, false, false, false),
    };

    Ok(SpaceDetailsDto {
        id: space_id.to_string(),
        name,
        topic,
        avatar_url,
        canonical_alias,
        member_count,
        join_rule,
        profile,
        can_edit_name,
        can_edit_topic,
        can_edit_avatar,
        can_edit_profile,
        can_edit_access,
        can_manage_roles,
        can_kick,
        can_ban,
        can_change_power,
    })
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
            tag: Some("UWU".into()),
            traits: vec![SpaceTrait { emoji: "🎮".into(), label: "gaming".into() }],
        };
        let json = serde_json::to_value(&profile).unwrap();
        let back: SpaceProfileDto = serde_json::from_value(json).unwrap();
        assert_eq!(back, profile);
    }
}
