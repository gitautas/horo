//! The emoji picker, as EmojiPicker.tsx draws it: a search box, a rail of
//! categories, recent reactions and the Unicode set in a grid of eight, and
//! a footer naming what's under the pointer with the skin-tone swatches.
//!
//! Its owner places it (in a `deferred(anchored())` at the button that
//! opened it) and listens for [`PickerEvent`]: a pick, or a close (Escape,
//! or a press anywhere else). Your custom packs come first, then the
//! stickers when the owner can send one (the composer; a reaction can't be
//! a sticker), then the Unicode set; search covers shortcodes and names.

use std::rc::Rc;

use gpui::{AppContext as _, StyledImage as _};

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Hsla, InteractiveElement, IntoElement, KeyDownEvent, ListAlignment,
    ListOffset, ListState, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, list, prelude::FluentBuilder,
    px,
};
use gpui_component::{
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};

use crate::{
    emoji::{self, SkinTone, TONE_SAMPLES},
    media::Media,
    packs::{self, Picked},
    store::Store,
    theme::{Palette, corner, space, text},
    views::voice::icon,
};

pub const WIDTH: f32 = 340.;
pub const HEIGHT: f32 = 392.;
const COLUMNS: usize = 8;
const CELL: f32 = 34.;
const SEARCH_LIMIT: usize = 90;
const EMOTE_SEARCH_LIMIT: usize = 16;
/// The size pack images are drawn at in the grid.
const CELL_IMAGE: f32 = 26.;

pub enum PickerEvent {
    Pick(Picked),
    Close,
}

#[derive(Clone)]
enum Draw {
    Text(String),
    /// An `mxc://` picture.
    Image(String),
}

#[derive(Clone)]
struct Cell {
    draw: Draw,
    /// What picking it sends.
    pick: Picked,
    /// What the footer says on hover.
    name: SharedString,
}

impl Cell {
    fn unicode(shown: String, name: &str) -> Cell {
        Cell { draw: Draw::Text(shown.clone()), pick: Picked::Unicode(shown), name: name.to_owned().into() }
    }

    fn image(image: &horo_core::matrix::packs::PackImageDto, sticker: bool) -> Cell {
        let pick = if sticker { Picked::Sticker(image.clone()) } else { Picked::Emote(image.clone()) };
        Cell { draw: Draw::Image(image.url.clone()), pick, name: format!(":{}:", image.shortcode).into() }
    }
}

/// A cell's face: the character, or the picture (blank while it loads).
fn face(draw: &Draw, size: f32, cx: &mut gpui::App) -> AnyElement {
    match draw {
        Draw::Text(text) => div().child(text.clone()).into_any_element(),
        Draw::Image(mxc) => match Media::image(mxc, Some((64, 64)), cx) {
            Some(image) => gpui::img(image).size(px(size)).object_fit(gpui::ObjectFit::Contain).into_any_element(),
            None => div().size(px(size)).into_any_element(),
        },
    }
}

enum Row {
    Title(SharedString),
    Cells(Vec<Cell>),
    Nothing(SharedString),
}

pub struct EmojiPicker {
    store: Entity<Store>,
    search: Entity<InputState>,
    list: ListState,
    rows: Rc<Vec<Row>>,
    /// The rail: each category's icon, title and first row.
    sections: Vec<(&'static str, SharedString, usize)>,
    hovered: Option<Cell>,
    focus: FocusHandle,
    /// Whether stickers are on offer (the composer) or not (reactions).
    stickers: bool,
}

impl EventEmitter<PickerEvent> for EmojiPicker {}

impl EmojiPicker {
    pub fn new(store: Entity<Store>, stickers: bool, window: &mut Window, cx: &mut Context<Self>) -> EmojiPicker {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("search emoji~"));
        cx.subscribe(&search, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.rebuild(cx);
            }
        })
        .detach();
        // A tone change redraws every hand.
        cx.observe(&store, |this, _, cx| this.rebuild(cx)).detach();
        search.update(cx, |s, cx| s.focus(window, cx));
        let mut picker = EmojiPicker {
            store,
            search,
            list: ListState::new(0, ListAlignment::Top, px(300.)),
            rows: Rc::new(Vec::new()),
            sections: Vec::new(),
            hovered: None,
            focus: cx.focus_handle(),
            stickers,
        };
        picker.rebuild(cx);
        picker
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().trim().to_owned();
        let (tone, recent, pack_list) = {
            let store = self.store.read(cx);
            (store.prefs.skin_tone as SkinTone, store.prefs.recent_reactions.clone(), store.packs.clone())
        };
        let known = packs::image_lookup(&pack_list);
        let mut rows = Vec::new();
        let mut sections = Vec::new();
        let grid = |rows: &mut Vec<Row>, cells: Vec<Cell>| {
            for chunk in cells.chunks(COLUMNS) {
                rows.push(Row::Cells(chunk.to_vec()));
            }
        };
        if query.is_empty() {
            if !recent.is_empty() {
                rows.push(Row::Title("recent".into()));
                // Stored as sent: already toned, and a custom emote as its
                // address (or a shortcode another client used).
                let cells = recent
                    .iter()
                    .map(|key| match packs::reaction_image(key, &known) {
                        Some((url, name)) => {
                            let pick = match packs::reaction_shortcode(key).and_then(|code| known.get(code)) {
                                Some(image) => Picked::Emote(image.clone()),
                                None => Picked::Unicode(key.clone()),
                            };
                            Cell { draw: Draw::Image(url), pick, name: name.into() }
                        }
                        None => Cell::unicode(key.clone(), emoji::name_of(key).unwrap_or(key)),
                    })
                    .collect();
                grid(&mut rows, cells);
            }
            for pack in &pack_list {
                let images: Vec<Cell> = pack.images.iter().filter(|i| i.is_emoticon).map(|i| Cell::image(i, false)).collect();
                if images.is_empty() {
                    continue;
                }
                let icon = if pack.source == horo_core::matrix::packs::PackSource::User { "icons/circle-user.svg" } else { "icons/users.svg" };
                sections.push((icon, SharedString::from(pack.display_name.clone()), rows.len()));
                rows.push(Row::Title(pack.display_name.clone().into()));
                grid(&mut rows, images);
            }
            if self.stickers {
                let stickers: Vec<Cell> = packs::stickers_of(&pack_list).iter().map(|i| Cell::image(i, true)).collect();
                if !stickers.is_empty() {
                    sections.push(("icons/sticker.svg", "stickers".into(), rows.len()));
                    rows.push(Row::Title("stickers".into()));
                    grid(&mut rows, stickers);
                }
            }
            for group in emoji::groups() {
                sections.push((group.icon, SharedString::from(group.name.clone()), rows.len()));
                rows.push(Row::Title(group.name.clone().into()));
                grid(&mut rows, group.emojis.iter().map(|e| Cell::unicode(emoji::toned(e, tone), e.name)).collect());
            }
        } else {
            let emotes = packs::match_emotes(&packs::emote_lookup(&pack_list), &query, EMOTE_SEARCH_LIMIT);
            let found = emoji::search(&query, SEARCH_LIMIT);
            if emotes.is_empty() && found.is_empty() {
                rows.push(Row::Nothing(format!("nothing matches \"{query}\"").into()));
            }
            if !emotes.is_empty() {
                rows.push(Row::Title("your packs".into()));
                grid(&mut rows, emotes.iter().map(|i| Cell::image(i, false)).collect());
            }
            if !found.is_empty() {
                rows.push(Row::Title(format!("{} emoji", found.len()).into()));
                grid(&mut rows, found.into_iter().map(|e| Cell::unicode(emoji::toned(e, tone), e.name)).collect());
            }
        }
        // A rebuild for a tone change keeps the scroll; a new query starts
        // at the top.
        let same_shape = rows.len() == self.rows.len();
        self.rows = Rc::new(rows);
        self.sections = sections;
        if !same_shape {
            self.list.reset(self.rows.len());
        } else {
            self.list.splice(0..self.rows.len(), self.rows.len());
        }
        cx.notify();
    }

    fn jump_to(&mut self, row: usize, cx: &mut Context<Self>) {
        self.list.scroll_to(ListOffset { item_ix: row, offset_in_item: px(0.) });
        cx.notify();
    }

    fn render_row(&mut self, ix: usize, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.rows.clone();
        match rows.get(ix) {
            Some(Row::Title(title)) => div()
                .px(space(4.))
                .pt(space(6.))
                .pb(space(4.))
                .font_family(palette.mono_font.clone())
                .text_size(text(10.))
                .text_color(palette.text_tertiary)
                .child(title.to_uppercase())
                .into_any_element(),
            Some(Row::Nothing(words)) => {
                div().py(space(28.)).px(space(12.)).text_center().text_size(text(12.5)).text_color(palette.text_tertiary).child(words.clone()).into_any_element()
            }
            Some(Row::Cells(cells)) => {
                let mut row = h_flex().w_full().gap(space(1.));
                for (col, cell) in cells.iter().enumerate() {
                    let (pick, hover) = (cell.pick.clone(), cell.clone());
                    let shown = face(&cell.draw, CELL_IMAGE, cx);
                    row = row.child(
                        div()
                            .id(("emoji", ix * COLUMNS + col))
                            .flex_1()
                            .h(px(CELL))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(corner(8.))
                            .cursor_pointer()
                            .text_size(text(20.))
                            .hover(|d| d.bg(Hsla { a: 0.08, ..gpui::white() }))
                            .child(shown)
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if *hovered {
                                    this.hovered = Some(hover.clone());
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(PickerEvent::Pick(pick.clone())))),
                    );
                }
                // A short last row keeps its cells the grid's width.
                for _ in cells.len()..COLUMNS {
                    row = row.child(div().flex_1().h(px(CELL)));
                }
                row.into_any_element()
            }
            None => div().into_any_element(),
        }
    }

    fn footer(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let tone = self.store.read(cx).prefs.skin_tone;
        let (shown, label) = match &self.hovered {
            Some(cell) => (face(&cell.draw, 19., cx), cell.name.clone()),
            None => (div().child("🫧").into_any_element(), "pick something~".into()),
        };
        let mut swatches = h_flex().gap(space(1.));
        for (i, sample) in TONE_SAMPLES.iter().enumerate() {
            let store = self.store.clone();
            swatches = swatches.child(
                div()
                    .id(("tone", i))
                    .size(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(corner(6.))
                    .cursor_pointer()
                    .text_size(text(12.))
                    .when(tone as usize == i, |d| d.bg(Hsla { a: 0.22, ..palette.accent }))
                    .tooltip(move |window, cx| {
                        let words = if i == 0 { "default skin tone".to_owned() } else { format!("skin tone {i}") };
                        gpui_component::tooltip::Tooltip::new(words).build(window, cx)
                    })
                    .child(*sample)
                    .on_click(move |_, _, cx| store.update(cx, |s, cx| s.update_prefs(|p| p.skin_tone = i as u8, cx))),
            );
        }
        h_flex()
            .gap(space(8.))
            .px(space(12.))
            .py(space(7.))
            .border_t_1()
            .border_color(palette.border_subtle)
            .bg(palette.card)
            .child(div().w(px(20.)).flex().justify_center().text_size(text(17.)).child(shown))
            .child(div().flex_1().min_w_0().truncate().text_size(text(11.5)).text_color(palette.text_secondary).child(label))
            .child(swatches)
            .into_any_element()
    }
}

impl Render for EmojiPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Rc::new(cx.global::<Palette>().clone());
        let searching = !self.search.read(cx).value().trim().is_empty();

        let mut rail = h_flex().gap(space(2.)).px(space(10.)).pb(space(6.)).border_b_1().border_color(palette.border_subtle);
        for (i, (path, title, row)) in self.sections.clone().into_iter().enumerate() {
            rail = rail.child(
                div()
                    .id(("rail", i))
                    .flex_1()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(corner(8.))
                    .cursor_pointer()
                    .hover(|d| d.bg(Hsla { a: 0.07, ..gpui::white() }))
                    .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(title.clone()).build(window, cx))
                    .child(icon(path).size(px(14.)).text_color(palette.text_tertiary))
                    .on_click(cx.listener(move |this, _, _, cx| this.jump_to(row, cx))),
            );
        }

        let this = cx.weak_entity();
        let palette_for_rows = palette.clone();
        let grid = list(self.list.clone(), move |ix, _, cx| {
            this.update(cx, |picker, cx| picker.render_row(ix, &palette_for_rows, cx)).unwrap_or_else(|_| div().into_any_element())
        })
        .flex_1()
        .min_h_0()
        .w_full()
        .px(space(8.))
        .pb(space(8.))
        .pt(space(4.));

        v_flex()
            .id("emoji-picker")
            // Clicks stop here rather than reaching the timeline under it.
            .occlude()
            .track_focus(&self.focus)
            .w(px(WIDTH))
            .h(px(HEIGHT))
            .overflow_hidden()
            .rounded(corner(20.))
            .border_1()
            .border_color(palette.border)
            .bg(palette.raised)
            .shadow_lg()
            .text_color(palette.text)
            .font_family(palette.body_font.clone())
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(PickerEvent::Close)))
            .capture_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    cx.emit(PickerEvent::Close);
                }
            }))
            .child(
                div().px(space(12.)).pt(space(10.)).pb(space(8.)).child(
                    h_flex()
                        .gap(space(8.))
                        .px(space(11.))
                        .py(space(6.))
                        .rounded_full()
                        .bg(palette.card)
                        .border_1()
                        .border_color(palette.border_subtle)
                        .child(icon("icons/search.svg").size(px(13.)).text_color(palette.text_tertiary))
                        .child(div().flex_1().min_w_0().text_size(text(13.)).child(Input::new(&self.search).appearance(false))),
                ),
            )
            .when(!searching, |d| d.child(rail))
            .child(grid)
            .child(self.footer(&palette, cx))
    }
}
