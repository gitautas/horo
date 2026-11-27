//! Full Space Settings floating modal (sidebar navigation + live preview card):
//! - Space Profile: name, description, icon upload/remove, banner presets, traits, space tag
//! - People: Members (power levels, kick, ban), Roles (`im.horo.roles`), Access (`m.room.join_rules` + invite link)
//! - Moderation: Bans (list & unban)

use gpui::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Window, div, linear_color_stop, linear_gradient, prelude::FluentBuilder,
};
use gpui_component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    v_flex,
};
use horo_core::{
    dto::RoomMemberDto,
    matrix::{
        roles::{self, RoleDto},
        rooms,
        spaces::{self, SpaceDetailsDto, SpaceProfileDto, SpaceTrait},
    },
};
use matrix_sdk::ruma::OwnedRoomId;

use crate::theme::zpx;
use crate::{
    backend::Backend,
    store::Store,
    theme::{Palette, corner, heading, label, space, text},
    views::{
        avatar::{accent_for, avatar},
        explore::Closed,
        modal::{accent_button, well},
    },
};

/// 10 Discord-inspired banner gradient presets: (id, top_rgb, bottom_rgb).
pub const BANNER_PRESETS: [(&str, u32, u32); 10] = [
    ("ice", 0x93C5FD, 0xE0F2FE),
    ("pink", 0xEC4899, 0xFF70B8),
    ("crimson", 0xDC2626, 0xF87171),
    ("sunset", 0xEA580C, 0xFDBA74),
    ("gold", 0xEAB308, 0xFEF08A),
    ("purple", 0x7E22CE, 0xC084FC),
    ("azure", 0x2563EB, 0x38BDF8),
    ("mint", 0x0D9488, 0x5EEAD4),
    ("olive", 0x4D7C0F, 0xA3E635),
    ("graphite", 0x27272A, 0x52525B),
];

pub fn banner_gradient(id: &str) -> Option<(u32, u32)> {
    BANNER_PRESETS.iter().find(|(k, _, _)| *k == id).map(|(_, a, b)| (*a, *b))
}

const ROLE_COLORS: [u32; 8] = [
    0x5865F2, // blurple
    0xEB459E, // fuchsia
    0xED4245, // coral
    0xE67E22, // tangerine
    0xFEE75C, // gold
    0x57F287, // mint
    0x1ABC9C, // cyan
    0x9B59B6, // lavender
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SpaceSection {
    Profile,
    Members,
    Roles,
    Access,
    Bans,
}

pub struct SpaceSettings {
    store: Entity<Store>,
    space_id: String,
    space_name: String,
    focus: FocusHandle,
    section: SpaceSection,
    details: Option<SpaceDetailsDto>,
    members: Option<Vec<RoomMemberDto>>,
    bans: Option<Vec<RoomMemberDto>>,
    /// Profile tab inputs & draft state.
    name: Entity<InputState>,
    topic: Entity<TextareaState>,
    tag: Entity<InputState>,
    traits: [Entity<InputState>; 5],
    banner: Option<String>,
    profile_dirty: bool,
    saving_profile: bool,
    uploading_avatar: bool,
    /// Roles tab state.
    selected_role: Option<String>,
    new_role_name: Entity<InputState>,
    new_role_color: u32,
    saving_roles: bool,
    /// Access tab state.
    alias_input: Entity<InputState>,
    link_base_input: Entity<InputState>,
    saving_alias: bool,
    saving_link_base: bool,
    copied_link: bool,
    /// Feedback.
    error: Option<SharedString>,
}

impl EventEmitter<Closed> for SpaceSettings {}

impl SpaceSettings {
    pub fn new(store: Entity<Store>, space_id: String, space_name: String, section: SpaceSection, window: &mut Window, cx: &mut Context<Self>) -> SpaceSettings {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("space name").default_value(space_name.clone()));
        let topic = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx);
            s.set_auto_grow(2, 5, cx);
            s.set_placeholder("how did this space get started? why should people join?", window, cx);
            s
        });
        let tag = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. UWU (max 4 chars)"));
        let traits = [
            cx.new(|cx| InputState::new(window, cx).placeholder("🎮 gaming")),
            cx.new(|cx| InputState::new(window, cx).placeholder("🎵 music")),
            cx.new(|cx| InputState::new(window, cx).placeholder("💻 rust")),
            cx.new(|cx| InputState::new(window, cx).placeholder("☕ chill")),
            cx.new(|cx| InputState::new(window, cx).placeholder("✨ art")),
        ];
        let new_role_name = cx.new(|cx| InputState::new(window, cx).placeholder("new role name (e.g. Gamers)"));
        let alias_input = cx.new(|cx| InputState::new(window, cx).placeholder("my-space"));
        let link_base_input = cx.new(|cx| InputState::new(window, cx).placeholder("https://matrix.to/#/ (or your domain)"));

        cx.subscribe(&name, |this, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                this.profile_dirty = true;
            }
            cx.notify();
        })
        .detach();
        cx.subscribe(&topic, |this, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                this.profile_dirty = true;
            }
            cx.notify();
        })
        .detach();
        cx.subscribe(&tag, |this, _, e: &InputEvent, cx| {
            if matches!(e, InputEvent::Change) {
                this.profile_dirty = true;
            }
            cx.notify();
        })
        .detach();
        for t in &traits {
            cx.subscribe(t, |this, _, e: &InputEvent, cx| {
                if matches!(e, InputEvent::Change) {
                    this.profile_dirty = true;
                }
                cx.notify();
            })
            .detach();
        }
        cx.subscribe(&new_role_name, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.subscribe(&alias_input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.subscribe(&link_base_input, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        cx.observe(&store, |_, _, cx| cx.notify()).detach();

        store.update(cx, |s, cx| {
            s.load_roles(&space_id, true, cx);
            s.load_space_profile(&space_id, true, cx);
        });

        let focus = cx.focus_handle();
        focus.focus(window, cx);

        let view = SpaceSettings {
            store,
            space_id,
            space_name,
            focus,
            section,
            details: None,
            members: None,
            bans: None,
            name,
            topic,
            tag,
            traits,
            banner: None,
            profile_dirty: false,
            saving_profile: false,
            uploading_avatar: false,
            selected_role: None,
            new_role_name,
            new_role_color: ROLE_COLORS[0],
            saving_roles: false,
            alias_input,
            link_base_input,
            saving_alias: false,
            saving_link_base: false,
            copied_link: false,
            error: None,
        };
        view.load_all(window, cx);
        view
    }

    fn load_all(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                let details = spaces::details(&core, &space).await?;
                let members = rooms::members(&core, &space).await.unwrap_or_default();
                let bans = spaces::banned_members(&core, &space).await.unwrap_or_default();
                Ok::<_, horo_core::Error>((details, members, bans))
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok((details, members, bans))) = task.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.name.update(cx, |s, cx| s.set_value(details.name.clone(), window, cx));
                    this.topic.update(cx, |s, cx| s.set_value(details.topic.clone(), window, cx));
                    this.tag.update(cx, |s, cx| s.set_value(details.profile.tag.clone().unwrap_or_default(), window, cx));
                    for (ix, input) in this.traits.iter().enumerate() {
                        let val = details
                            .profile
                            .traits
                            .get(ix)
                            .map(|t| if t.emoji.is_empty() { t.label.clone() } else { format!("{} {}", t.emoji, t.label) })
                            .unwrap_or_default();
                        input.update(cx, |s, cx| s.set_value(val, window, cx));
                    }
                    this.banner = details.profile.banner.clone();
                    let alias_local = details
                        .canonical_alias
                        .as_deref()
                        .and_then(|a| a.strip_prefix('#'))
                        .and_then(|a| a.split(':').next())
                        .unwrap_or("")
                        .to_owned();
                    this.alias_input.update(cx, |s, cx| s.set_value(alias_local, window, cx));
                    this.link_base_input.update(cx, |s, cx| s.set_value(details.profile.link_base.clone().unwrap_or_default(), window, cx));
                    this.space_name = details.name.clone();
                    this.profile_dirty = false;
                    this.details = Some(details);
                    this.members = Some(members.into_iter().filter(|m| m.membership == "join").collect());
                    this.bans = Some(bans);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn draft_traits(&self, cx: &Context<Self>) -> Vec<SpaceTrait> {
        self.traits
            .iter()
            .filter_map(|input| {
                let raw = input.read(cx).value().trim().to_owned();
                if raw.is_empty() {
                    return None;
                }
                let mut parts = raw.splitn(2, char::is_whitespace);
                let first = parts.next().unwrap_or("").trim();
                let rest = parts.next().map(str::trim).unwrap_or("");
                if !rest.is_empty() && first.chars().count() <= 2 {
                    Some(SpaceTrait { emoji: first.to_owned(), label: rest.to_owned() })
                } else {
                    Some(SpaceTrait { emoji: String::new(), label: raw })
                }
            })
            .collect()
    }

    fn save_profile(&mut self, cx: &mut Context<Self>) {
        if self.saving_profile {
            return;
        }
        let Some(details) = self.details.clone() else { return };
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };

        let new_name = self.name.read(cx).value().trim().to_owned();
        let new_topic = self.topic.read(cx).value().trim().to_owned();
        let name_change = (details.can_edit_name && !new_name.is_empty() && new_name != details.name).then_some(new_name.clone());
        let topic_change = (details.can_edit_topic && new_topic != details.topic).then_some(new_topic.clone());

        let tag_raw = self.tag.read(cx).value().trim().chars().take(4).collect::<String>();
        let profile = SpaceProfileDto {
            banner: self.banner.clone(),
            tag: (!tag_raw.is_empty()).then_some(tag_raw),
            traits: self.draft_traits(cx),
            link_base: details.profile.link_base.clone(),
        };
        let profile_change = (details.can_edit_profile && profile != details.profile).then_some(profile.clone());

        if name_change.is_none() && topic_change.is_none() && profile_change.is_none() {
            self.profile_dirty = false;
            cx.notify();
            return;
        }

        self.saving_profile = true;
        self.error = None;
        cx.notify();

        let space_id = self.space_id.clone();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move {
                let core = state.core().await?;
                if name_change.is_some() || topic_change.is_some() {
                    rooms::update(&core, &space, name_change, topic_change).await?;
                }
                if let Some(p) = profile_change {
                    spaces::save_profile(&core, &space, &p).await?;
                }
                Ok::<_, horo_core::Error>(())
            }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                this.saving_profile = false;
                match error {
                    None => {
                        this.profile_dirty = false;
                        this.space_name = new_name.clone();
                        if let Some(d) = this.details.as_mut() {
                            d.name = new_name;
                            d.topic = new_topic;
                            d.profile = profile.clone();
                        }
                        this.store.update(cx, |s, cx| {
                            s.set_space_profile_cached(space_id, profile, cx);
                            s.refresh_spaces_now(cx);
                        });
                    }
                    Some(e) => {
                        this.error = Some(format!("couldn't save space profile: {e}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn pick_avatar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.uploading_avatar {
            return;
        }
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("choose space icon".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let Ok(bytes) = std::fs::read(&path) else { return };
            let mime = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
                Some("png") => "image/png",
                Some("jpg" | "jpeg") => "image/jpeg",
                Some("webp") => "image/webp",
                Some("gif") => "image/gif",
                _ => "image/png",
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let Ok(space) = OwnedRoomId::try_from(this.space_id.as_str()) else { return };
                this.uploading_avatar = true;
                this.error = None;
                cx.notify();
                let backend = Backend::global(cx);
                let task = backend.run({
                    let state = backend.state.clone();
                    async move { spaces::upload_avatar(&*state.core().await?, &space, bytes, mime).await }
                });
                cx.spawn_in(window, async move |this, cx| {
                    let error = match task.await {
                        Ok(Ok(())) => None,
                        Ok(Err(e)) => Some(e.to_string()),
                        Err(e) => Some(e.to_string()),
                    };
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.uploading_avatar = false;
                        if let Some(e) = error {
                            this.error = Some(format!("couldn't upload icon: {e}").into());
                        } else {
                            this.store.update(cx, |s, cx| s.refresh_spaces_now(cx));
                            this.load_all(window, cx);
                        }
                        cx.notify();
                    });
                })
                .detach();
            });
        })
        .detach();
    }

    fn remove_avatar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.uploading_avatar {
            return;
        }
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        self.uploading_avatar = true;
        self.error = None;
        cx.notify();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { spaces::remove_avatar(&*state.core().await?, &space).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.uploading_avatar = false;
                if let Some(e) = error {
                    this.error = Some(format!("couldn't remove icon: {e}").into());
                } else {
                    this.store.update(cx, |s, cx| s.refresh_spaces_now(cx));
                    this.load_all(window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_alias(&mut self, cx: &mut Context<Self>) {
        if self.saving_alias {
            return;
        }
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        let localpart = self.alias_input.read(cx).value().trim().to_owned();
        self.saving_alias = true;
        self.error = None;
        cx.notify();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { spaces::set_canonical_alias(&*state.core().await?, &space, &localpart).await }
        });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |this, cx| {
                this.saving_alias = false;
                match outcome {
                    Ok(Ok(alias)) => {
                        if let Some(d) = this.details.as_mut() {
                            d.canonical_alias = (!alias.is_empty()).then_some(alias);
                        }
                        this.copied_link = false;
                    }
                    Ok(Err(e)) => this.error = Some(format!("couldn't update alias: {e}").into()),
                    Err(e) => this.error = Some(format!("couldn't update alias: {e}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_link_base(&mut self, cx: &mut Context<Self>) {
        if self.saving_link_base {
            return;
        }
        let Some(details) = self.details.clone() else { return };
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        let raw = self.link_base_input.read(cx).value().trim().to_owned();
        let mut profile = details.profile.clone();
        profile.link_base = (!raw.is_empty()).then_some(raw);

        self.saving_link_base = true;
        self.error = None;
        cx.notify();
        let space_id = self.space_id.clone();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            let p = profile.clone();
            async move { spaces::save_profile(&*state.core().await?, &space, &p).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                this.saving_link_base = false;
                match error {
                    None => {
                        if let Some(d) = this.details.as_mut() {
                            d.profile = profile.clone();
                        }
                        this.copied_link = false;
                        this.store.update(cx, |s, cx| s.set_space_profile_cached(space_id, profile, cx));
                    }
                    Some(e) => this.error = Some(format!("couldn't save link domain: {e}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_join_rule(&mut self, rule: &'static str, cx: &mut Context<Self>) {
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        if let Some(d) = self.details.as_mut() {
            d.join_rule = rule.to_owned();
        }
        self.error = None;
        cx.notify();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { spaces::set_join_rule(&*state.core().await?, &space, rule).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            if let Some(e) = error {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(format!("couldn't update join rule: {e}").into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn set_power(&mut self, user_id: String, level: i64, cx: &mut Context<Self>) {
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        if let Some(members) = self.members.as_mut()
            && let Some(m) = members.iter_mut().find(|m| m.user_id == user_id)
        {
            m.power_level = level;
        }
        cx.notify();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { spaces::set_member_power(&*state.core().await?, &space, &user_id, level).await }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(format!("couldn't change power level: {e}").into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn kick(&mut self, user_id: String, cx: &mut Context<Self>) {
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        if let Some(members) = self.members.as_mut() {
            members.retain(|m| m.user_id != user_id);
        }
        cx.notify();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { spaces::kick_member(&*state.core().await?, &space, &user_id).await }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(format!("couldn't kick member: {e}").into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn ban(&mut self, user_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        if let Some(members) = self.members.as_mut() {
            members.retain(|m| m.user_id != user_id);
        }
        cx.notify();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { spaces::ban_member(&*state.core().await?, &space, &user_id).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let err = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(e) = err {
                    this.error = Some(format!("couldn't ban member: {e}").into());
                } else {
                    this.load_all(window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn unban(&mut self, user_id: String, cx: &mut Context<Self>) {
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        if let Some(bans) = self.bans.as_mut() {
            bans.retain(|m| m.user_id != user_id);
        }
        cx.notify();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { spaces::unban_member(&*state.core().await?, &space, &user_id).await }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(format!("couldn't unban member: {e}").into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn persist_roles(&mut self, updated: Vec<RoleDto>, cx: &mut Context<Self>) {
        let Ok(space) = OwnedRoomId::try_from(self.space_id.as_str()) else { return };
        let space_id = self.space_id.clone();
        self.store.update(cx, |s, cx| s.set_roles_cached(space_id, updated.clone(), cx));
        self.saving_roles = true;
        self.error = None;
        cx.notify();
        let backend = Backend::global(cx);
        let task = backend.run({
            let state = backend.state.clone();
            async move { roles::save(&*state.core().await?, &space, &updated).await }
        });
        cx.spawn(async move |this, cx| {
            let error = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                this.saving_roles = false;
                if let Some(err) = error {
                    this.error = Some(format!("couldn't save roles: {err}").into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn create_role(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_role_name.read(cx).value().trim().to_owned();
        let can_manage = self.details.as_ref().is_some_and(|d| d.can_manage_roles);
        if name.is_empty() || !can_manage {
            return;
        }
        let mut current = self.store.read(cx).roles_for_space(&self.space_id).to_vec();
        let base_id = roles::slug(&name);
        let mut id = base_id.clone();
        let mut suffix = 2usize;
        while current.iter().any(|r| r.id == id) {
            id = format!("{base_id}-{suffix}");
            suffix += 1;
        }
        current.push(RoleDto {
            id: id.clone(),
            name,
            color: self.new_role_color,
            mentionable: true,
            hoist: true,
            members: Vec::new(),
        });
        self.new_role_name.update(cx, |s, cx| s.set_value("", window, cx));
        self.selected_role = Some(id);
        self.persist_roles(current, cx);
    }

    fn delete_role(&mut self, role_id: &str, cx: &mut Context<Self>) {
        if !self.details.as_ref().is_some_and(|d| d.can_manage_roles) {
            return;
        }
        let mut current = self.store.read(cx).roles_for_space(&self.space_id).to_vec();
        current.retain(|r| r.id != role_id);
        if self.selected_role.as_deref() == Some(role_id) {
            self.selected_role = current.first().map(|r| r.id.clone());
        }
        self.persist_roles(current, cx);
    }

    fn update_role(&mut self, role_id: &str, mutate: impl FnOnce(&mut RoleDto), cx: &mut Context<Self>) {
        if !self.details.as_ref().is_some_and(|d| d.can_manage_roles) {
            return;
        }
        let mut current = self.store.read(cx).roles_for_space(&self.space_id).to_vec();
        if let Some(role) = current.iter_mut().find(|r| r.id == role_id) {
            mutate(role);
            self.persist_roles(current, cx);
        }
    }

    fn profile_pane(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let details = self.details.clone();
        let can_edit_name = details.as_ref().is_some_and(|d| d.can_edit_name);
        let can_edit_topic = details.as_ref().is_some_and(|d| d.can_edit_topic);
        let can_edit_avatar = details.as_ref().is_some_and(|d| d.can_edit_avatar);
        let can_edit_profile = details.as_ref().is_some_and(|d| d.can_edit_profile);
        let avatar_url = details.as_ref().and_then(|d| d.avatar_url.clone());
        let member_count = details.as_ref().map_or(0, |d| d.member_count);
        let live_name = {
            let v = self.name.read(cx).value().trim().to_owned();
            if v.is_empty() { self.space_name.clone() } else { v }
        };
        let live_tag = self.tag.read(cx).value().trim().chars().take(4).collect::<String>();
        let live_traits = self.draft_traits(cx);

        let preview_banner = match self.banner.as_deref().and_then(banner_gradient) {
            Some((a, b)) => div().h(zpx(86.)).w_full().bg(linear_gradient(135., linear_color_stop(gpui::rgb(a), 0.), linear_color_stop(gpui::rgb(b), 1.))),
            None => div().h(zpx(86.)).w_full().bg(palette.raised),
        };

        let preview_card = v_flex()
            .flex_none()
            .w(zpx(230.))
            .rounded(corner(16.))
            .overflow_hidden()
            .bg(palette.app)
            .border_1()
            .border_color(palette.border_subtle)
            .child(preview_banner)
            .child(
                v_flex()
                    .px(space(14.))
                    .pb(space(14.))
                    .child(
                        div()
                            .mt(zpx(-28.))
                            .size(zpx(56.))
                            .rounded(corner(16.))
                            .p(zpx(3.))
                            .bg(palette.app)
                            .child(avatar(&self.space_id, &live_name, avatar_url.as_deref(), zpx(50.), cx)),
                    )
                    .child(
                        h_flex()
                            .mt(zpx(8.))
                            .gap(space(6.))
                            .items_center()
                            .child(div().truncate().font_family(palette.heading_font.clone()).font_weight(FontWeight::EXTRA_BOLD).text_size(heading(15.)).text_color(palette.text).child(live_name))
                            .when(!live_tag.is_empty(), |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .px(space(6.))
                                        .py(zpx(1.))
                                        .rounded(corner(6.))
                                        .bg(Hsla { a: 0.16, ..palette.accent })
                                        .font_family(palette.mono_font.clone())
                                        .text_size(text(10.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(palette.accent)
                                        .child(live_tag),
                                )
                            }),
                    )
                    .child(
                        div()
                            .mt(zpx(4.))
                            .text_size(text(11.5))
                            .text_color(palette.text_tertiary)
                            .child(format!("{member_count} {}", if member_count == 1 { "member" } else { "members" })),
                    )
                    .when(!live_traits.is_empty(), |col| {
                        col.child(
                            h_flex()
                                .mt(zpx(10.))
                                .flex_wrap()
                                .gap(zpx(5.))
                                .children(live_traits.into_iter().map(|t| {
                                    div()
                                        .px(space(7.))
                                        .py(zpx(2.))
                                        .rounded_full()
                                        .bg(palette.card)
                                        .border_1()
                                        .border_color(palette.border_subtle)
                                        .text_size(text(10.5))
                                        .text_color(palette.text_secondary)
                                        .child(if t.emoji.is_empty() { t.label } else { format!("{} {}", t.emoji, t.label) })
                                })),
                        )
                    }),
            );

        let form = v_flex()
            .flex_1()
            .min_w_0()
            .gap(space(16.))
            .child(
                v_flex()
                    .gap(zpx(4.))
                    .child(div().font_family(palette.heading_font.clone()).font_weight(FontWeight::EXTRA_BOLD).text_size(heading(17.)).child("space profile"))
                    .child(div().text_size(text(12.)).text_color(palette.text_tertiary).child("customize how this space looks in horo and in the sidebar.")),
            )
            .child(
                v_flex()
                    .gap(zpx(6.))
                    .child(section_label("name", palette))
                    .child(well(palette).child(div().flex_1().child(Input::new(&self.name).appearance(false).disabled(!can_edit_name)))),
            )
            .child(
                v_flex()
                    .gap(zpx(6.))
                    .child(section_label("icon", palette))
                    .child(
                        h_flex()
                            .gap(space(8.))
                            .child(
                                accent_button("space-icon-upload", if self.uploading_avatar { "uploading…" } else { "change space icon" }, can_edit_avatar && !self.uploading_avatar, true, palette)
                                    .on_click(cx.listener(|this, _, window, cx| this.pick_avatar(window, cx))),
                            )
                            .when(avatar_url.is_some() && can_edit_avatar, |row| {
                                row.child(
                                    Button::new("space-icon-remove")
                                        .ghost()
                                        .small()
                                        .label("remove icon")
                                        .disabled(self.uploading_avatar)
                                        .on_click(cx.listener(|this, _, window, cx| this.remove_avatar(window, cx))),
                                )
                            }),
                    ),
            )
            .child(
                v_flex()
                    .gap(zpx(8.))
                    .child(section_label("banner", palette))
                    .child(
                        h_flex().flex_wrap().gap(space(8.)).children(BANNER_PRESETS.into_iter().enumerate().map(|(ix, (id, a, b))| {
                            let active = self.banner.as_deref() == Some(id);
                            div()
                                .id(("banner-swatch", ix))
                                .w(zpx(68.))
                                .h(zpx(40.))
                                .rounded(corner(10.))
                                .bg(linear_gradient(135., linear_color_stop(gpui::rgb(a), 0.), linear_color_stop(gpui::rgb(b), 1.)))
                                .border_2()
                                .border_color(if active { palette.text } else { gpui::transparent_black() })
                                .when(can_edit_profile, |d| {
                                    d.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                                        this.banner = if this.banner.as_deref() == Some(id) { None } else { Some(id.to_owned()) };
                                        this.profile_dirty = true;
                                        cx.notify();
                                    }))
                                })
                        })),
                    ),
            )
            .child(
                v_flex()
                    .gap(zpx(6.))
                    .child(section_label("space tag", palette))
                    .child(well(palette).child(div().flex_1().child(Input::new(&self.tag).appearance(false).disabled(!can_edit_profile)))),
            )
            .child(
                v_flex()
                    .gap(zpx(6.))
                    .child(section_label("traits (up to 5)", palette))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap(space(6.))
                            .children(self.traits.iter().map(|t| {
                                div().w(zpx(140.)).child(well(palette).child(div().flex_1().child(Input::new(t).appearance(false).disabled(!can_edit_profile))))
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap(zpx(6.))
                    .child(section_label("description", palette))
                    .child(well(palette).child(div().flex_1().child(Textarea::new(&self.topic).appearance(false).disabled(!can_edit_topic)))),
            )
            .when(self.profile_dirty, |col| {
                col.child(
                    h_flex()
                        .px(space(12.))
                        .py(space(8.))
                        .rounded(corner(14.))
                        .bg(palette.app)
                        .border_1()
                        .border_color(palette.accent)
                        .items_center()
                        .justify_between()
                        .child(div().text_size(text(12.)).text_color(palette.text).child("careful — you have unsaved changes!"))
                        .child(
                            accent_button("space-profile-save", if self.saving_profile { "saving…" } else { "save changes" }, !self.saving_profile, true, palette)
                                .on_click(cx.listener(|this, _, _, cx| this.save_profile(cx))),
                        ),
                )
            });

        h_flex().items_start().gap(space(20.)).child(form).child(preview_card).into_any_element()
    }

    fn members_pane(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let details = self.details.clone();
        let can_kick = details.as_ref().is_some_and(|d| d.can_kick);
        let can_ban = details.as_ref().is_some_and(|d| d.can_ban);
        let can_power = details.as_ref().is_some_and(|d| d.can_change_power);
        let own_id = self.store.read(cx).own_user_id().map(str::to_owned);
        let members = self.members.clone().unwrap_or_default();

        v_flex()
            .gap(space(12.))
            .child(
                v_flex()
                    .gap(zpx(4.))
                    .child(div().font_family(palette.heading_font.clone()).font_weight(FontWeight::EXTRA_BOLD).text_size(heading(17.)).child(format!("members · {}", members.len())))
                    .child(div().text_size(text(12.)).text_color(palette.text_tertiary).child("manage space power levels and moderation.")),
            )
            .child(v_flex().gap(space(6.)).children(members.into_iter().enumerate().map(|(ix, member)| {
                let is_me = own_id.as_deref() == Some(member.user_id.as_str());
                let name = member.display_name.clone().unwrap_or_else(|| member.user_id.clone());
                let role_label = if member.power_level >= 100 {
                    "admin"
                } else if member.power_level >= 50 {
                    "mod"
                } else {
                    "member"
                };
                let uid_mod = member.user_id.clone();
                let uid_kick = member.user_id.clone();
                let uid_ban = member.user_id.clone();
                let next_level = if member.power_level >= 50 { 0 } else { 50 };
                h_flex()
                    .id(("sp-mem", ix))
                    .items_center()
                    .justify_between()
                    .gap(space(10.))
                    .px(space(10.))
                    .py(space(8.))
                    .rounded(corner(12.))
                    .bg(palette.app)
                    .border_1()
                    .border_color(palette.border_subtle)
                    .child(
                        h_flex()
                            .gap(space(10.))
                            .min_w_0()
                            .child(avatar(&member.user_id, &name, member.avatar_url.as_deref(), zpx(30.), cx))
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .child(div().truncate().text_size(text(13.)).font_weight(FontWeight::BOLD).text_color(accent_for(&member.user_id)).child(name))
                                    .child(div().truncate().font_family(palette.mono_font.clone()).text_size(text(10.5)).text_color(palette.text_tertiary).child(member.user_id.clone())),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(space(6.))
                            .items_center()
                            .child(
                                div()
                                    .px(space(7.))
                                    .py(zpx(2.))
                                    .rounded(corner(6.))
                                    .bg(palette.card)
                                    .font_family(palette.mono_font.clone())
                                    .text_size(text(10.5))
                                    .text_color(if member.power_level >= 50 { palette.accent } else { palette.text_tertiary })
                                    .child(role_label),
                            )
                            .when(!is_me && can_power && member.power_level < 100, |row| {
                                row.child(
                                    Button::new(("sp-mod", ix))
                                        .ghost()
                                        .small()
                                        .label(if member.power_level >= 50 { "demote" } else { "make mod" })
                                        .on_click(cx.listener(move |this, _, _, cx| this.set_power(uid_mod.clone(), next_level, cx))),
                                )
                            })
                            .when(!is_me && can_kick && member.power_level < 100, |row| {
                                row.child(
                                    Button::new(("sp-kick", ix))
                                        .ghost()
                                        .small()
                                        .label("kick")
                                        .on_click(cx.listener(move |this, _, _, cx| this.kick(uid_kick.clone(), cx))),
                                )
                            })
                            .when(!is_me && can_ban && member.power_level < 100, |row| {
                                row.child(
                                    Button::new(("sp-ban", ix))
                                        .danger()
                                        .small()
                                        .label("ban")
                                        .on_click(cx.listener(move |this, _, window, cx| this.ban(uid_ban.clone(), window, cx))),
                                )
                            }),
                    )
            })))
            .into_any_element()
    }

    fn roles_pane(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let can_manage = self.details.as_ref().is_some_and(|d| d.can_manage_roles);
        let roles = self.store.read(cx).roles_for_space(&self.space_id).to_vec();
        let selected_id = self
            .selected_role
            .clone()
            .filter(|id| roles.iter().any(|r| &r.id == id))
            .or_else(|| roles.first().map(|r| r.id.clone()));
        let selected = selected_id.as_deref().and_then(|id| roles.iter().find(|r| r.id == id).cloned());
        let can_create = can_manage && !self.new_role_name.read(cx).value().trim().is_empty() && !self.saving_roles;

        let create_box = can_manage.then(|| {
            v_flex()
                .gap(space(8.))
                .child(section_label("create role", palette))
                .child(
                    h_flex()
                        .gap(space(8.))
                        .items_center()
                        .child(div().flex_1().child(well(palette).child(div().flex_1().child(Input::new(&self.new_role_name).appearance(false)))))
                        .child(accent_button("role-create", "create", can_create, true, palette).on_click(cx.listener(|this, _, window, cx| this.create_role(window, cx)))),
                )
                .child(h_flex().gap(space(6.)).children(ROLE_COLORS.into_iter().enumerate().map(|(ix, hex)| {
                    let picked = self.new_role_color == hex;
                    let c: Hsla = gpui::rgb(hex).into();
                    div()
                        .id(("new-role-color", ix))
                        .size(zpx(18.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(c)
                        .when(picked, |d| d.border_2().border_color(palette.text))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.new_role_color = hex;
                            cx.notify();
                        }))
                })))
        });

        let list = if roles.is_empty() {
            div()
                .p(space(14.))
                .rounded(corner(12.))
                .bg(palette.app)
                .border_1()
                .border_color(palette.border_subtle)
                .text_size(text(12.))
                .text_color(palette.text_tertiary)
                .child("no roles yet — create one above to color names, group member lists, and mention @RoleName across every room in this space~")
                .into_any_element()
        } else {
            h_flex()
                .flex_wrap()
                .gap(space(6.))
                .children(roles.iter().enumerate().map(|(ix, role)| {
                    let active = selected_id.as_deref() == Some(role.id.as_str());
                    let c: Hsla = gpui::rgb(role.color).into();
                    let select_id = role.id.clone();
                    let del_id = role.id.clone();
                    h_flex()
                        .id(("role-pill", ix))
                        .items_center()
                        .gap(space(6.))
                        .pl(space(10.))
                        .pr(space(if can_manage { 6. } else { 10. }))
                        .py(space(5.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(if active { Hsla { a: 0.16, ..c } } else { palette.app })
                        .border_1()
                        .border_color(if active { c } else { palette.border_subtle })
                        .child(div().size(zpx(8.)).rounded_full().bg(c))
                        .child(div().text_size(text(12.)).font_weight(FontWeight::BOLD).text_color(if active { c } else { palette.text }).child(format!("@{}", role.name)))
                        .child(div().font_family(palette.mono_font.clone()).text_size(text(10.)).text_color(palette.text_tertiary).child(role.members.len().to_string()))
                        .when(can_manage, |d| {
                            d.child(
                                div()
                                    .id(("role-del", ix))
                                    .px(zpx(4.))
                                    .rounded_full()
                                    .text_size(text(10.))
                                    .text_color(palette.text_tertiary)
                                    .hover(|d| d.text_color(palette.danger))
                                    .child("✕")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.delete_role(&del_id, cx);
                                    })),
                            )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_role = Some(select_id.clone());
                            cx.notify();
                        }))
                }))
                .into_any_element()
        };

        let detail = selected.map(|role| {
            let role_id = role.id.clone();
            let c: Hsla = gpui::rgb(role.color).into();
            let members_list = self.members.clone().unwrap_or_default();
            let toggle_chip = |id: &'static str, label_text: &'static str, on: bool| {
                div()
                    .id(id)
                    .px(space(10.))
                    .py(space(5.))
                    .rounded(corner(10.))
                    .border_1()
                    .border_color(if on { palette.accent } else { palette.border_subtle })
                    .bg(if on { Hsla { a: 0.16, ..palette.accent } } else { palette.card })
                    .text_size(text(11.5))
                    .font_weight(if on { FontWeight::BOLD } else { FontWeight::MEDIUM })
                    .text_color(if on { palette.accent } else { palette.text_secondary })
                    .when(can_manage, |d| d.cursor_pointer())
                    .child(label_text)
            };
            let rid_m = role_id.clone();
            let rid_h = role_id.clone();
            v_flex()
                .p(space(12.))
                .gap(space(10.))
                .rounded(corner(14.))
                .bg(palette.app)
                .border_1()
                .border_color(palette.border_subtle)
                .child(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .child(
                            h_flex()
                                .gap(space(8.))
                                .items_center()
                                .child(div().size(zpx(10.)).rounded_full().bg(c))
                                .child(div().text_size(text(13.)).font_weight(FontWeight::BOLD).text_color(c).child(format!("@{}", role.name))),
                        )
                        .when(can_manage, |d| {
                            d.child(h_flex().gap(zpx(5.)).children(ROLE_COLORS.into_iter().enumerate().map(|(ix, hex)| {
                                let rid = role_id.clone();
                                let swatch: Hsla = gpui::rgb(hex).into();
                                div()
                                    .id(("edit-role-color", ix))
                                    .size(zpx(14.))
                                    .rounded_full()
                                    .cursor_pointer()
                                    .bg(swatch)
                                    .when(role.color == hex, |d| d.border_2().border_color(palette.text))
                                    .on_click(cx.listener(move |this, _, _, cx| this.update_role(&rid, |r| r.color = hex, cx)))
                            })))
                        }),
                )
                .child(
                    h_flex()
                        .gap(space(8.))
                        .child(toggle_chip("role-mentionable", if role.mentionable { "mentionable ✓" } else { "not mentionable" }, role.mentionable).when(can_manage, |d| {
                            d.on_click(cx.listener(move |this, _, _, cx| this.update_role(&rid_m, |r| r.mentionable = !r.mentionable, cx)))
                        }))
                        .child(toggle_chip("role-hoist", if role.hoist { "grouped in sidebar ✓" } else { "not grouped" }, role.hoist).when(can_manage, |d| {
                            d.on_click(cx.listener(move |this, _, _, cx| this.update_role(&rid_h, |r| r.hoist = !r.hoist, cx)))
                        })),
                )
                .child(section_label("space members with this role", palette))
                .child(v_flex().gap(zpx(4.)).children(members_list.into_iter().enumerate().map(|(ix, member)| {
                    let has = role.members.iter().any(|id| id == &member.user_id);
                    let name = member.display_name.clone().unwrap_or_else(|| member.user_id.clone());
                    let uid = member.user_id.clone();
                    let rid = role_id.clone();
                    h_flex()
                        .id(("role-member", ix))
                        .items_center()
                        .justify_between()
                        .gap(space(8.))
                        .px(space(8.))
                        .py(space(5.))
                        .rounded(corner(10.))
                        .bg(if has { Hsla { a: 0.12, ..c } } else { palette.card })
                        .when(can_manage, |d| d.cursor_pointer().hover(|d| d.bg(palette.raised)))
                        .child(
                            h_flex()
                                .gap(space(8.))
                                .min_w_0()
                                .child(avatar(&member.user_id, &name, member.avatar_url.as_deref(), zpx(22.), cx))
                                .child(div().truncate().text_size(text(12.)).text_color(if has { palette.text } else { palette.text_secondary }).child(name)),
                        )
                        .child(div().flex_none().text_size(text(11.)).font_weight(FontWeight::BOLD).text_color(if has { c } else { palette.text_tertiary }).child(if has { "✓" } else { "+" }))
                        .when(can_manage, |d| {
                            d.on_click(cx.listener(move |this, _, _, cx| {
                                let uid = uid.clone();
                                this.update_role(
                                    &rid,
                                    move |r| {
                                        if let Some(pos) = r.members.iter().position(|id| id == &uid) {
                                            r.members.remove(pos);
                                        } else {
                                            r.members.push(uid);
                                        }
                                    },
                                    cx,
                                );
                            }))
                        })
                })))
        });

        v_flex()
            .gap(space(14.))
            .child(
                v_flex()
                    .gap(zpx(4.))
                    .child(div().font_family(palette.heading_font.clone()).font_weight(FontWeight::EXTRA_BOLD).text_size(heading(17.)).child("roles"))
                    .child(div().text_size(text(12.)).text_color(palette.text_tertiary).child("roles apply across every room in this space and expand into matrix mentions.")),
            )
            .children(create_box)
            .child(list)
            .children(detail)
            .into_any_element()
    }

    fn access_pane(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let details = self.details.clone();
        let can_edit = details.as_ref().is_some_and(|d| d.can_edit_access);
        let can_edit_alias = details.as_ref().is_some_and(|d| d.can_edit_alias);
        let can_edit_profile = details.as_ref().is_some_and(|d| d.can_edit_profile);
        let current = details.as_ref().map(|d| d.join_rule.as_str()).unwrap_or("invite");
        let domain = details.as_ref().map(|d| d.homeserver_domain.clone()).unwrap_or_default();
        let address = details.as_ref().and_then(|d| d.canonical_alias.clone()).unwrap_or_else(|| self.space_id.clone());
        let live_base = self.link_base_input.read(cx).value().trim().to_owned();
        let link = spaces::space_invite_url((!live_base.is_empty()).then_some(live_base.as_str()), &address);
        let copy_link = link.clone();

        let rules = [
            ("invite", "invite only", "only people who are invited can join this space."),
            ("knock", "request to join", "anyone with the link can ask an admin to let them in."),
            ("public", "public", "anyone on matrix can find and join this space."),
        ];

        v_flex()
            .gap(space(16.))
            .child(
                v_flex()
                    .gap(zpx(4.))
                    .child(div().font_family(palette.heading_font.clone()).font_weight(FontWeight::EXTRA_BOLD).text_size(heading(17.)).child("access & invites"))
                    .child(div().text_size(text(12.)).text_color(palette.text_tertiary).child("control who can join this matrix space and customize its shareable address.")),
            )
            .child(
                v_flex().gap(space(8.)).child(section_label("join rule (m.room.join_rules)", palette)).children(rules.into_iter().enumerate().map(|(ix, (id, title, desc))| {
                    let active = current == id;
                    v_flex()
                        .id(("join-rule", ix))
                        .gap(zpx(2.))
                        .p(space(12.))
                        .rounded(corner(12.))
                        .bg(if active { Hsla { a: 0.14, ..palette.accent } } else { palette.app })
                        .border_1()
                        .border_color(if active { palette.accent } else { palette.border_subtle })
                        .when(can_edit, |d| d.cursor_pointer().hover(|d| d.bg(palette.raised)))
                        .child(div().text_size(text(13.)).font_weight(FontWeight::BOLD).text_color(if active { palette.accent } else { palette.text }).child(title))
                        .child(div().text_size(text(11.5)).text_color(palette.text_secondary).child(desc))
                        .when(can_edit, |d| d.on_click(cx.listener(move |this, _, _, cx| this.set_join_rule(id, cx))))
                })),
            )
            .child(
                v_flex()
                    .gap(zpx(6.))
                    .child(section_label("space address (m.room.canonical_alias)", palette))
                    .child(
                        h_flex()
                            .gap(space(8.))
                            .items_center()
                            .child(
                                div().flex_1().child(
                                    well(palette)
                                        .child(div().flex_none().font_family(palette.mono_font.clone()).text_color(palette.text_tertiary).child("#"))
                                        .child(div().flex_1().child(Input::new(&self.alias_input).appearance(false).disabled(!can_edit_alias)))
                                        .when(!domain.is_empty(), |w| {
                                            w.child(div().flex_none().font_family(palette.mono_font.clone()).text_size(text(11.5)).text_color(palette.text_tertiary).child(format!(":{domain}")))
                                        }),
                                ),
                            )
                            .when(can_edit_alias, |row| {
                                row.child(
                                    accent_button("save-space-alias", if self.saving_alias { "saving…" } else { "save alias" }, !self.saving_alias, true, palette)
                                        .on_click(cx.listener(|this, _, _, cx| this.save_alias(cx))),
                                )
                            }),
                    ),
            )
            .child(
                v_flex()
                    .gap(zpx(6.))
                    .child(section_label("custom invite link domain", palette))
                    .child(
                        h_flex()
                            .gap(space(8.))
                            .items_center()
                            .child(div().flex_1().child(well(palette).child(div().flex_1().child(Input::new(&self.link_base_input).appearance(false).disabled(!can_edit_profile)))))
                            .when(can_edit_profile, |row| {
                                row.child(
                                    accent_button("save-link-base", if self.saving_link_base { "saving…" } else { "save" }, !self.saving_link_base, true, palette)
                                        .on_click(cx.listener(|this, _, _, cx| this.save_link_base(cx))),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(text(11.))
                            .text_color(palette.text_tertiary)
                            .child(format!("leave blank for matrix.to, or enter e.g. https://{}/#/", if domain.is_empty() { "your.homeserver" } else { &domain })),
                    ),
            )
            .child(
                v_flex()
                    .gap(zpx(6.))
                    .child(section_label("space link", palette))
                    .child(
                        h_flex()
                            .gap(space(8.))
                            .items_center()
                            .p(space(10.))
                            .rounded(corner(12.))
                            .bg(palette.app)
                            .border_1()
                            .border_color(palette.border_subtle)
                            .child(div().flex_1().truncate().font_family(palette.mono_font.clone()).text_size(text(11.5)).text_color(palette.text_secondary).child(link))
                            .child(
                                accent_button("copy-space-link", if self.copied_link { "copied!" } else { "copy link" }, true, true, palette).on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_link.clone()));
                                    this.copied_link = true;
                                    cx.notify();
                                })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn bans_pane(&mut self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let can_ban = self.details.as_ref().is_some_and(|d| d.can_ban);
        let bans = self.bans.clone().unwrap_or_default();
        v_flex()
            .gap(space(12.))
            .child(
                v_flex()
                    .gap(zpx(4.))
                    .child(div().font_family(palette.heading_font.clone()).font_weight(FontWeight::EXTRA_BOLD).text_size(heading(17.)).child(format!("bans · {}", bans.len())))
                    .child(div().text_size(text(12.)).text_color(palette.text_tertiary).child("people banned from this space.")),
            )
            .child(if bans.is_empty() {
                div()
                    .p(space(16.))
                    .rounded(corner(12.))
                    .bg(palette.app)
                    .border_1()
                    .border_color(palette.border_subtle)
                    .text_size(text(12.))
                    .text_color(palette.text_tertiary)
                    .child("nobody is banned from this space~")
                    .into_any_element()
            } else {
                v_flex()
                    .gap(space(6.))
                    .children(bans.into_iter().enumerate().map(|(ix, member)| {
                        let name = member.display_name.clone().unwrap_or_else(|| member.user_id.clone());
                        let uid = member.user_id.clone();
                        h_flex()
                            .id(("sp-ban-row", ix))
                            .items_center()
                            .justify_between()
                            .px(space(10.))
                            .py(space(8.))
                            .rounded(corner(12.))
                            .bg(palette.app)
                            .border_1()
                            .border_color(palette.border_subtle)
                            .child(
                                h_flex()
                                    .gap(space(10.))
                                    .min_w_0()
                                    .child(avatar(&member.user_id, &name, member.avatar_url.as_deref(), zpx(28.), cx))
                                    .child(
                                        v_flex()
                                            .min_w_0()
                                            .child(div().truncate().text_size(text(12.5)).font_weight(FontWeight::BOLD).text_color(palette.text).child(name))
                                            .child(div().truncate().font_family(palette.mono_font.clone()).text_size(text(10.5)).text_color(palette.text_tertiary).child(member.user_id)),
                                    ),
                            )
                            .when(can_ban, |row| {
                                row.child(Button::new(("sp-unban", ix)).ghost().small().label("unban").on_click(cx.listener(move |this, _, _, cx| this.unban(uid.clone(), cx))))
                            })
                    }))
                    .into_any_element()
            })
            .into_any_element()
    }
}

fn section_label(title: &'static str, palette: &Palette) -> AnyElement {
    div()
        .font_family(palette.heading_font.clone())
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(label(9.5))
        .text_color(palette.text_tertiary)
        .child(title)
        .into_any_element()
}

impl Render for SpaceSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.global::<Palette>().clone();
        let nav_item = |section: SpaceSection, title: &'static str, cx: &mut Context<Self>| {
            let active = self.section == section;
            div()
                .id(SharedString::from(format!("sp-nav-{title}")))
                .px(space(10.))
                .py(space(7.))
                .rounded(corner(9.))
                .cursor_pointer()
                .text_size(text(12.5))
                .font_weight(if active { FontWeight::BOLD } else { FontWeight::MEDIUM })
                .bg(if active { palette.raised } else { gpui::transparent_black() })
                .text_color(if active { palette.text } else { palette.text_secondary })
                .when(!active, |d| d.hover(|d| d.bg(Hsla { a: 0.05, ..gpui::white() }).text_color(palette.text)))
                .child(title)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.section = section;
                    this.error = None;
                    cx.notify();
                }))
        };

        let sidebar = v_flex()
            .flex_none()
            .w(zpx(188.))
            .h_full()
            .p(space(14.))
            .gap(zpx(4.))
            .bg(palette.app)
            .border_r_1()
            .border_color(palette.border_subtle)
            .child(div().px(space(10.)).pb(space(8.)).truncate().font_family(palette.heading_font.clone()).font_weight(FontWeight::EXTRA_BOLD).text_size(heading(14.)).text_color(palette.text).child(self.space_name.clone()))
            .child(nav_item(SpaceSection::Profile, "space profile", cx))
            .child(div().px(space(10.)).pt(space(12.)).pb(zpx(4.)).child(section_label("people", &palette)))
            .child(nav_item(SpaceSection::Members, "members", cx))
            .child(nav_item(SpaceSection::Roles, "roles", cx))
            .child(nav_item(SpaceSection::Access, "access & invites", cx))
            .child(div().px(space(10.)).pt(space(12.)).pb(zpx(4.)).child(section_label("moderation", &palette)))
            .child(nav_item(SpaceSection::Bans, "bans", cx));

        let content = match self.section {
            SpaceSection::Profile => self.profile_pane(&palette, cx),
            SpaceSection::Members => self.members_pane(&palette, cx),
            SpaceSection::Roles => self.roles_pane(&palette, cx),
            SpaceSection::Access => self.access_pane(&palette, cx),
            SpaceSection::Bans => self.bans_pane(&palette, cx),
        };

        div()
            .id("space-settings-backdrop")
            .occlude()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(Hsla { a: 0.56, ..gpui::black() })
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(Closed);
                }
            }))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(Closed)))
            .child(
                h_flex()
                    .id("space-settings-card")
                    .w(zpx(860.))
                    .max_w(gpui::relative(0.92))
                    .h(zpx(610.))
                    .max_h(gpui::relative(0.88))
                    .rounded(corner(20.))
                    .overflow_hidden()
                    .bg(palette.card)
                    .border_1()
                    .border_color(palette.border)
                    .shadow_lg()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(sidebar)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .relative()
                            .child(
                                div()
                                    .id("space-settings-close")
                                    .absolute()
                                    .top(space(14.))
                                    .right(space(16.))
                                    .size(zpx(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .bg(palette.app)
                                    .border_1()
                                    .border_color(palette.border_subtle)
                                    .text_size(text(11.))
                                    .text_color(palette.text_tertiary)
                                    .hover(|d| d.bg(palette.raised).text_color(palette.text))
                                    .child("✕")
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Closed))),
                            )
                            .child(
                                v_flex()
                                    .id("space-settings-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .p(space(24.))
                                    .pr(space(48.))
                                    .child(content)
                                    .children(self.error.clone().map(|e| div().mt(space(12.)).text_size(text(12.)).text_color(palette.danger).child(e))),
                            ),
                    ),
            )
    }
}
