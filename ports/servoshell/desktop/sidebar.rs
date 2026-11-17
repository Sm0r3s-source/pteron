/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Sirocco: a Zen-style vertical sidebar. It floats over the page (the page is never
//! shifted or resized) and slides in when the pointer touches the window edge (left or right).
//! This module only knows about egui; `gui.rs` turns the returned [`Action`]s into
//! servoshell commands.

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{
    Align2, Area, Button, Color32, Context, FontId, Frame, Id, Key, Modifiers, Order, Pos2, Rect,
    RichText, ScrollArea, Sense, Shape, Stroke, TextEdit, pos2, vec2,
};

/// Width of the invisible strip at the window edge that belongs to the sidebar while it is hidden.
pub(crate) const HOT_ZONE: f32 = 4.0;
/// How near the edge the mouse has to come for the sidebar to open (less on the right, where
/// pages have their scroll bar), and how long it stays after the mouse has left it.
const OPEN_ZONE: (f32, f32) = (16.0, 6.0);
const LINGER_SECS: f64 = 0.8;
pub(crate) const LOCATION_ID: &str = "location_input";

thread_local! {
    /// Height of a tab or bookmark row (from the interface density setting), and whether every
    /// tab shows its close button.
    static ROW: std::cell::Cell<(f32, bool)> = const { std::cell::Cell::new((28.0, false)) };
}

pub(crate) fn row_height() -> f32 {
    ROW.with(|row| row.get().0)
}

pub(crate) fn close_always() -> bool {
    ROW.with(|row| row.get().1)
}

pub(crate) fn set_row_style(height: f32, close_always: bool) {
    ROW.with(|row| row.set((height, close_always)));
}

/// One live tab, as shown in the list.
pub(crate) struct TabRow {
    pub label: String,
    pub active: bool,
    pub pinned: bool,
    pub favicon: Option<egui::load::SizedTexture>,
    /// Kept awake by the user (never goes to sleep).
    pub awake: bool,
    /// One of the two tabs of the split view.
    pub split: bool,
    /// Playing sound (audio or video) right now.
    pub audible: bool,
    /// Muted by the user.
    pub muted: bool,
    /// A private (incognito) tab.
    pub private: bool,
}

/// One bookmark, as shown in the list.
pub(crate) struct BookmarkRow {
    pub label: String,
    pub url: String,
    pub folder: String,
}

/// One sleeping tab, as shown in the list.
pub(crate) struct SleepingRow {
    pub label: String,
    pub pinned: bool,
}

/// Everything the sidebar needs to draw itself.
/// What the address bar says about the connection of the current page.
#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) enum Security {
    /// Internal pages, new tab, local files: nothing to say.
    #[default]
    None,
    /// https
    Secure,
    /// plain http
    NotSecure,
}

pub(crate) struct Model {
    pub security: Security,
    pub tabs: Vec<TabRow>,
    /// Sleeping tabs.
    pub sleeping: Vec<SleepingRow>,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub loading: bool,
    pub experimental: bool,
    pub bookmarks: Vec<BookmarkRow>,
    /// Whether the page in the active tab is bookmarked, and whether it can be at all.
    pub bookmarked: bool,
    pub can_bookmark: bool,
    /// Pages matching what is being typed in the address bar.
    pub suggestions: Vec<crate::library::Suggestion>,
    /// With dark mode for pages on and a web page showing: whether it is dark here.
    pub dark_here: Option<bool>,
    /// Sirocco draws the window's own top: minimize, maximize and close buttons are needed.
    pub own_titlebar: bool,
    /// The active tab is a private one.
    pub private: bool,
}

/// The purple that marks private tabs.
pub(crate) const PRIVATE: Color32 = Color32::from_rgb(160, 115, 235);

/// The "Private" badge shown next to the address bar of a private tab.
pub(crate) fn private_pill(ui: &mut egui::Ui) {
    Frame::NONE
        .fill(PRIVATE.gamma_multiply(0.22))
        .corner_radius(11)
        .inner_margin(egui::Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(format!("Private{}", crate::tor::badge_suffix())).size(12.0).strong().color(PRIVATE));
            if crate::tor::status() == crate::tor::Status::Starting {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(600));
            }
        });
}

/// What the user did in the sidebar this frame.
pub(crate) enum Action {
    Back,
    Forward,
    Reload,
    Go(String),
    NewTab,
    NewPrivateTab,
    NewWindow,
    Activate(usize),
    Close(usize),
    /// Drag a tab: move tab `from` to where tab `to` is.
    MoveTab { from: usize, to: usize },
    Wake(usize),
    CloseSleeping(usize),
    TogglePin(usize),
    /// Open this tab's page in the computer's default browser.
    OpenExternal(usize),
    CheckPage(usize),
    TogglePinSleeping(usize),
    ToggleBookmark,
    /// Show the page in reading mode.
    ReaderMode,
    OpenBookmark(usize),
    OpenBookmarkInNewTab(usize),
    RemoveBookmark(usize),
    /// Put a bookmark into a folder (empty name: no folder).
    MoveBookmark(usize, String),
    /// Drag a bookmark: move bookmark `from` to where bookmark `to` is.
    MoveBookmarkTo { from: usize, to: usize },
    SetExperimental(bool),
    ToggleSettings,
    /// Go to the new tab page.
    Home,
    /// Switch dark mode off (or back on) for the site in the active tab.
    ToggleDarkSite,
    /// Show or hide the bookmarks bar (top-tab layouts).
    ToggleBookmarksBar,
    ReloadTab(usize),
    /// Open the tab's page in another tab.
    Duplicate(usize),
    /// Keep the tab awake (it never goes to sleep), or stop doing so.
    ToggleAwake(usize),
    /// Split view: show this tab beside the active tab.
    SplitWith(usize),
    LeaveSplit,
    /// Close every tab except this one, or every tab listed after it (pinned tabs stay).
    CloseOthers(usize),
    CloseAfter(usize),
    /// Mute or unmute the tab's sound.
    ToggleMute(usize),
    /// Stop loading the page in the active tab.
    Stop,
    /// Save a picture of the tab's page.
    Screenshot(usize),
    /// Open the browsing history.
    History,
    Window(WindowAction),
}

/// What Sirocco's own window top asks of the window.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum WindowAction {
    /// Start moving the window with the mouse.
    Drag,
    Minimize,
    /// Maximize, or back to the size before.
    Maximize,
    Close,
}

/// An empty part of Sirocco's own window top: drag it to move the window, double-click to
/// maximize. Call it before drawing what lies on top of it.
pub(crate) fn drag_area(ui: &egui::Ui, rect: Rect, actions: &mut Vec<Action>) {
    let response = ui.interact(rect, Id::new("pteron_window_drag"), Sense::click_and_drag());
    if response.double_clicked() {
        actions.push(Action::Window(WindowAction::Maximize));
    } else if response.drag_started() {
        actions.push(Action::Window(WindowAction::Drag));
    }
}

/// Minimize, maximize and close, drawn leftwards from `right_top`. Returns their width.
pub(crate) fn window_buttons(ui: &egui::Ui, right_top: Pos2, size: egui::Vec2, actions: &mut Vec<Action>) -> f32 {
    let ink = ui.visuals().text_color();
    let buttons = [(WindowAction::Close, "Close"), (WindowAction::Maximize, "Maximize"), (WindowAction::Minimize, "Minimize")];
    for (index, (action, name)) in buttons.into_iter().enumerate() {
        let rect = Rect::from_min_size(right_top - vec2(size.x * (index + 1) as f32, 0.0), size);
        let response = ui.interact(rect, Id::new(("pteron_window_button", index)), Sense::click()).on_hover_text(name);
        let painter = ui.painter();
        let mut color = ink;
        if response.hovered() {
            let close = action == WindowAction::Close;
            painter.rect_filled(rect, 0.0, if close { Color32::from_rgb(232, 17, 35) } else { ink.gamma_multiply(0.12) });
            if close {
                color = Color32::WHITE;
            }
        }
        let (c, stroke) = (rect.center(), Stroke::new(1.2_f32, color));
        match action {
            WindowAction::Minimize => {
                painter.line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], stroke);
            },
            WindowAction::Maximize => {
                painter.rect_stroke(Rect::from_center_size(c, vec2(10.0, 10.0)), 1.0, stroke, egui::StrokeKind::Middle);
            },
            _ => {
                painter.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], stroke);
                painter.line_segment([c + vec2(-5.0, 5.0), c + vec2(5.0, -5.0)], stroke);
            },
        }
        if response.clicked() {
            actions.push(Action::Window(action));
        }
    }
    size.x * 3.0
}

#[derive(Default)]
pub(crate) struct State {
    /// Whether the sidebar is currently shown.
    pub visible: bool,
    /// Keep the sidebar open all the time (comes from the settings).
    pub pinned: bool,
    /// Ask the address bar to take keyboard focus (Ctrl+L).
    pub focus_location: bool,
    /// Text typed in the "Filter tabs and bookmarks" box.
    pub filter: String,
    /// Width of the part of the window (from the left edge) that belongs to the sidebar
    /// right now, including the hot zone when hidden. Used to route mouse events.
    pub hit_width: f32,
    /// The mouse is over a compatibility tab's page (a native view that egui cannot see).
    pub page_hover: bool,
    /// The mouse is at the sidebar's edge of a compatibility tab's page.
    pub page_edge: bool,
    /// Width of the sidebar and the side it is on (from the settings).
    pub width: f32,
    pub right: bool,
    /// When the mouse left the open sidebar (it stays a moment longer).
    pub left_at: Option<f64>,
}

/// Whether `text` contains every word of `filter` (ignoring case). An empty filter matches all.
pub(crate) fn filter_matches(filter: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    filter.split_whitespace().all(|word| text.contains(&word.to_lowercase()))
}

/// Decide whether the sidebar is shown this frame.
pub(crate) fn update_visibility(ctx: &Context, state: &mut State) {
    let pointer_x = ctx.pointer_latest_pos().map(|pos| pos.x);
    let location_focused = ctx.memory(|memory| memory.has_focus(Id::new(LOCATION_ID)));
    let reach = if state.visible {
        state.width + 6.0
    } else if state.right {
        OPEN_ZONE.1
    } else {
        OPEN_ZONE.0
    };
    let right_edge = ctx.content_rect().max.x;
    let near = if state.page_edge {
        true
    } else if state.page_hover {
        false
    } else if state.right {
        pointer_x.is_some_and(|x| x >= right_edge - reach)
    } else {
        pointer_x.is_some_and(|x| x <= reach)
    };
    // The sidebar stays a moment after the mouse has left it.
    let now = ctx.input(|input| input.time);
    if near {
        state.left_at = None;
    } else if state.visible {
        let left_at = *state.left_at.get_or_insert(now);
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((left_at + LINGER_SECS - now).max(0.02)));
    }
    let lingering = state.left_at.is_some_and(|left_at| now - left_at < LINGER_SECS);
    let visible = state.pinned || state.focus_location || location_focused || near || lingering;
    if !visible {
        state.left_at = None;
    }
    if visible != state.visible {
        ctx.request_repaint();
    }
    state.visible = visible;
    state.hit_width = if visible { state.width } else { HOT_ZONE };
}

/// Width (in egui points) that a pinned sidebar takes from the left of the page. Read by the
/// window to move mouse positions and pop-ups, which are measured from the page's corner.
static LEFT_INSET_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub(crate) fn set_left_inset(points: f32) {
    LEFT_INSET_BITS.store(points.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn left_inset() -> f32 {
    f32::from_bits(LEFT_INSET_BITS.load(std::sync::atomic::Ordering::Relaxed))
}

/// How far the active tab's page starts to the right of the page area (the right-hand page of a
/// split view), in the same units as `left_inset`.
static PANE_INSET_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub(crate) fn set_pane_inset(points: f32) {
    PANE_INSET_BITS.store(points.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// Where the active page's own pop-ups (lists, menus, the typing aid) measure from.
pub(crate) fn popup_inset() -> f32 {
    left_inset() + f32::from_bits(PANE_INSET_BITS.load(std::sync::atomic::Ordering::Relaxed))
}

/// The interface size setting (1.0 = 100%): egui's points are this many screen points. Read by
/// the pop-ups of pages (lists, menus), whose positions come in screen points.
static UI_SCALE_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000);

pub(crate) fn set_ui_scale(scale: f32) {
    UI_SCALE_BITS.store(scale.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn ui_scale() -> f32 {
    f32::from_bits(UI_SCALE_BITS.load(std::sync::atomic::Ordering::Relaxed))
}

/// Is the address bar being edited right now?
pub(crate) fn location_focused(ctx: &Context) -> bool {
    ctx.memory(|memory| memory.has_focus(Id::new(LOCATION_ID)))
}

/// The accent colour of the chosen colour theme.
pub(crate) fn accent_color() -> Color32 {
    let [r, g, b] = crate::appearance::current_accent();
    Color32::from_rgb(r, g, b)
}

/// `base` with a little of `tint` mixed in (`amount` 0.0 to 1.0).
pub(crate) fn tinted(base: Color32, tint: Color32, amount: f32) -> Color32 {
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    Color32::from_rgb(mix(base.r(), tint.r()), mix(base.g(), tint.g()), mix(base.b(), tint.b()))
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Glyph {
    Previous,
    Next,
    Play,
    Pause,
}

/// A round media button with its symbol drawn by the painter. `main` is the big filled one.
pub(crate) fn media_button(ui: &mut egui::Ui, glyph: Glyph, main: bool, accent: Color32) -> egui::Response {
    let size = if main { 34.0 } else { 28.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let painter = ui.painter();
    let center = rect.center();
    let radius = size / 2.0;
    let (fill, ink) = if main {
        let fill = if response.hovered() { accent } else { accent.gamma_multiply(0.9) };
        (fill, Color32::from_rgb(24, 24, 26))
    } else {
        let fill = if response.hovered() {
            accent.gamma_multiply(0.30)
        } else {
            accent.gamma_multiply(0.14)
        };
        (fill, ui.visuals().text_color())
    };
    painter.circle_filled(center, radius, fill);
    let u = size / 34.0 * 6.5;
    let triangle = |tip_x: f32, back_x: f32| {
        Shape::convex_polygon(
            vec![
                center + vec2(tip_x, 0.0),
                center + vec2(back_x, -u),
                center + vec2(back_x, u),
            ],
            ink,
            Stroke::NONE,
        )
    };
    match glyph {
        Glyph::Play => {
            painter.add(triangle(u * 1.0 + 1.0, -u * 0.8 + 1.0));
        },
        Glyph::Pause => {
            for dx in [-u * 0.55, u * 0.55] {
                painter.rect_filled(
                    Rect::from_center_size(center + vec2(dx, 0.0), vec2(u * 0.55, u * 1.8)),
                    1.5,
                    ink,
                );
            }
        },
        Glyph::Next => {
            painter.add(triangle(u * 0.8, -u * 0.7));
            painter.rect_filled(
                Rect::from_center_size(center + vec2(u * 0.95, 0.0), vec2(u * 0.35, u * 1.7)),
                1.0,
                ink,
            );
        },
        Glyph::Previous => {
            painter.add(triangle(-u * 0.8, u * 0.7));
            painter.rect_filled(
                Rect::from_center_size(center + vec2(-u * 0.95, 0.0), vec2(u * 0.35, u * 1.7)),
                1.0,
                ink,
            );
        },
    }
    response
}

/// Three little bars, like a sound level meter.
pub(crate) fn equalizer_icon(ui: &mut egui::Ui, accent: Color32, playing: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
    let heights: [f32; 3] = if playing { [9.0, 14.0, 7.0] } else { [5.0, 5.0, 5.0] };
    for (i, h) in heights.iter().enumerate() {
        let x = rect.min.x + 2.0 + i as f32 * 5.0;
        ui.painter().rect_filled(
            Rect::from_min_max(pos2(x, rect.max.y - h), pos2(x + 3.0, rect.max.y)),
            1.5,
            accent,
        );
    }
}

/// A star (outlined, or filled when the page is bookmarked), drawn with the painter.
pub(crate) fn star_button(ui: &mut egui::Ui, filled: bool, enabled: bool) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(vec2(26.0, 26.0), sense);
    if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let gold = accent_color();
    let color = if !enabled {
        ui.visuals().weak_text_color()
    } else if filled {
        gold
    } else {
        ui.visuals().text_color()
    };
    let center = rect.center() + vec2(0.0, 0.5);
    let points: Vec<Pos2> = (0..10)
        .map(|i| {
            let angle = i as f32 * std::f32::consts::PI / 5.0 - std::f32::consts::FRAC_PI_2;
            let radius = if i % 2 == 0 { 9.0_f32 } else { 4.0_f32 };
            center + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    if filled && enabled {
        ui.painter()
            .add(Shape::convex_polygon(points, color, Stroke::new(1.0_f32, color)));
    } else {
        ui.painter()
            .add(Shape::closed_line(points, Stroke::new(1.4_f32, color)));
    }
    response
}

pub(crate) fn icon_button(text: &str) -> Button<'_> {
    Button::new(text).frame(false).min_size(vec2(26.0, 26.0))
}

/// A 26x26 button whose icon is drawn with the painter, so every icon in the row has the same weight.
fn painted_button(ui: &mut egui::Ui, draw: impl FnOnce(&egui::Painter, Pos2, egui::Color32)) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 8.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    draw(ui.painter(), rect.center(), ui.visuals().text_color());
    response
}

/// A clock: a circle with two hands.
pub(crate) fn clock_button(ui: &mut egui::Ui) -> egui::Response {
    painted_button(ui, |painter, c, color| {
        let stroke = Stroke::new(1.4_f32, color);
        painter.circle_stroke(c, 8.0, stroke);
        painter.line_segment([c, c + vec2(0.0, -5.0)], stroke);
        painter.line_segment([c, c + vec2(3.6, 2.0)], stroke);
    })
}

/// A window with a plus: open a new window.
pub(crate) fn window_button(ui: &mut egui::Ui) -> egui::Response {
    painted_button(ui, |painter, c, color| {
        let stroke = Stroke::new(1.4_f32, color);
        painter.rect_stroke(egui::Rect::from_center_size(c, vec2(16.0, 14.0)), 3.0, stroke, egui::StrokeKind::Inside);
        painter.line_segment([c + vec2(-3.5, 0.0), c + vec2(3.5, 0.0)], stroke);
        painter.line_segment([c + vec2(0.0, -3.5), c + vec2(0.0, 3.5)], stroke);
    })
}

/// A cog with triangular teeth, drawn with the painter (no font glyph needed).
pub(crate) fn cog_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let color = ui.visuals().text_color();
    let center = rect.center();
    // 8 triangular teeth on a round body: base-left, tip, base-right, valley.
    let teeth = 8;
    let step = std::f32::consts::TAU / teeth as f32;
    let (body, tip, half_width) = (7.0_f32, 10.4_f32, 0.27_f32);
    let at = |angle: f32, radius: f32| -> Pos2 {
        center + vec2(angle.cos(), angle.sin()) * radius
    };
    let mut points: Vec<Pos2> = Vec::with_capacity(teeth * 4);
    for i in 0..teeth {
        let angle = i as f32 * step - std::f32::consts::FRAC_PI_2;
        points.push(at(angle - half_width, body));
        points.push(at(angle, tip));
        points.push(at(angle + half_width, body));
        points.push(at(angle + step / 2.0, body));
    }
    let stroke = Stroke::new(1.6_f32, color);
    ui.painter().add(Shape::closed_line(points, stroke));
    ui.painter().circle_stroke(center, 3.0, stroke);
    response
}

/// Draw the sidebar (if visible) and report what was clicked.
/// Draw the sidebar and report what was clicked. While pinned it is a real panel that
/// takes space from the page; otherwise it floats over the page and only shows when the
/// pointer touches the left edge.
pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &Model,
    look: &crate::look::Look,
    location: &mut String,
    location_dirty: &mut bool,
    height: f32,
) -> Vec<Action> {
    let mut actions = Vec::new();
    // Forget a finished tab drag.
    if ui.input(|input| !input.pointer.any_down() && !input.pointer.any_released()) {
        ui.memory_mut(|memory| memory.data.remove::<usize>(Id::new("pteron_tab_drag")));
        ui.memory_mut(|memory| memory.data.remove::<usize>(Id::new("pteron_bookmark_drag")));
    }
    // A steady, clearly visible typing cursor. (The blinking one needs timed repaints, which
    // Sirocco does not run while idle, so it could stay invisible.)
    if ui.ctx().global_style().visuals.text_cursor.blink {
        ui.ctx().global_style_mut(|style| {
            style.visuals.text_cursor.blink = false;
            style.visuals.text_cursor.stroke = Stroke::new(2.0_f32, accent_color());
        });
    }
    if state.pinned {
        let panel = if state.right {
            egui::Panel::right("pteron_sidebar_panel")
        } else {
            egui::Panel::left("pteron_sidebar_panel")
        };
        panel
            .resizable(false)
            .exact_size(state.width)
            .frame(Frame::NONE)
            .show_inside(ui, |ui| {
                framed(ui, state, model, look, location, location_dirty, height, &mut actions);
            });
        return actions;
    }
    if !state.visible {
        // Sirocco draws the window's own top: with the sidebar tucked away, minimize, maximize and
        // close stay reachable in the top corner (faint until the mouse is on them).
        if model.own_titlebar {
            let ctx = ui.ctx().clone();
            let size = vec2(34.0, 26.0);
            let screen = ctx.content_rect();
            let strip = Rect::from_min_size(pos2(screen.max.x - size.x * 3.0, screen.min.y), vec2(size.x * 3.0, size.y));
            egui::Area::new(Id::new("pteron_corner_buttons"))
                .order(egui::Order::Foreground)
                .fixed_pos(strip.min)
                .show(&ctx, |ui| {
                    ui.set_min_size(strip.size());
                    let hovered = ui.rect_contains_pointer(strip);
                    ui.set_opacity(if hovered { 1.0 } else { 0.5 });
                    let fill = ui.visuals().window_fill();
                    ui.painter().rect_filled(strip, 0.0, fill.gamma_multiply(if hovered { 0.95 } else { 0.75 }));
                    window_buttons(ui, strip.right_top(), size, &mut actions);
                });
        }
        return actions;
    }
    let ctx = ui.ctx().clone();
    let x = if state.right { ctx.content_rect().max.x - state.width } else { 0.0 };
    Area::new(Id::new("pteron_sidebar"))
        .order(Order::Foreground)
        .fixed_pos(pos2(x, 0.0))
        .show(&ctx, |ui| {
            framed(ui, state, model, look, location, location_dirty, height, &mut actions);
        });
    actions
}

#[allow(clippy::too_many_arguments)]
fn framed(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &Model,
    look: &crate::look::Look,
    location: &mut String,
    location_dirty: &mut bool,
    height: f32,
    actions: &mut Vec<Action>,
) {
    let margin = 8.0;
    {
        {
            Frame::default()
                .fill(tinted(ui.visuals().window_fill, accent_color(), 0.06))
                .stroke(ui.visuals().window_stroke)
                .inner_margin(margin)
                .show(ui, |ui| {
                    ui.set_width(state.width - 2.0 * margin);
                    ui.set_min_height(height - 2.0 * margin);
                    let inner_width = ui.available_width();

                    // Sirocco's own window top: a strip to drag the window by, and its buttons.
                    if model.own_titlebar {
                        let (strip, _) = ui.allocate_exact_size(vec2(inner_width, 22.0), Sense::hover());
                        let strip = strip.expand2(vec2(margin, 0.0)).with_min_y(strip.min.y - margin);
                        drag_area(ui, strip, actions);
                        window_buttons(ui, strip.right_top(), vec2(34.0, strip.height()), actions);
                        ui.add_space(2.0);
                    }
                    // Navigation buttons.
                    ui.horizontal(|ui| {
                        nav_buttons(ui, model, look, location_dirty, actions);
                        page_buttons(ui, model, look, actions);
                    });
                    ui.add_space(4.0);

                    if model.private {
                        private_pill(ui);
                        ui.add_space(4.0);
                    }
                    // Address bar: a lock (or warning) at the left, a copy-link button at the right.
                    address_bar(ui, state, model, look, location, location_dirty, inner_width, 28.0, actions);
                    if model.security == Security::NotSecure {
                        ui.label(
                            RichText::new("Not secure (http) - do not enter passwords")
                                .small()
                                .color(Color32::from_rgb(220, 150, 40)),
                        );
                    }
                    ui.add_space(6.0);

                    if ui
                        .add_sized([inner_width, row_height()], Button::new("+  New tab"))
                        .clicked()
                    {
                        actions.push(Action::NewTab);
                    }
                    if ui
                        .add_sized([inner_width, row_height()], Button::new("+  New private tab"))
                        .clicked()
                    {
                        actions.push(Action::NewPrivateTab);
                    }
                    // Sirocco: what Plume Music is playing, with simple controls.
                    music_card(ui, inner_width);
                    ui.add_space(6.0);
                    let total = model.tabs.len() + model.sleeping.len() + model.bookmarks.len();
                    if total >= 6 || !state.filter.is_empty() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let clear_width = if state.filter.is_empty() { 0.0 } else { 26.0 };
                            ui.add_sized(
                                [inner_width - clear_width - 4.0, 24.0],
                                TextEdit::singleline(&mut state.filter).hint_text("Filter tabs and bookmarks"),
                            );
                            if !state.filter.is_empty() &&
                                ui.add_sized([22.0, 24.0], Button::new("\u{00D7}").frame(false)).on_hover_text("Clear filter").clicked()
                            {
                                state.filter.clear();
                            }
                        });
                    }
                    let filter = state.filter.trim().to_string();
                    ui.separator();

                    // Vertical tab list.
                    let list_height = (height - 190.0).max(60.0);
                    ScrollArea::vertical()
                        .max_height(list_height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            // Pinned tabs first, then the rest.
                            let any_pinned = model.tabs.iter().any(|tab| tab.pinned) ||
                                model.sleeping.iter().any(|tab| tab.pinned);
                            for section_pinned in [true, false] {
                                if section_pinned {
                                    if !any_pinned {
                                        continue;
                                    }
                                } else if any_pinned {
                                    ui.add_space(2.0);
                                    ui.separator();
                                    ui.add_space(2.0);
                                }
                                for (index, tab) in model.tabs.iter().enumerate() {
                                    if tab.pinned == section_pinned && filter_matches(&filter, &tab.label) {
                                        tab_row(ui, index, tab, model, actions);
                                    }
                                }
                                for (index, tab) in model.sleeping.iter().enumerate() {
                                    if tab.pinned == section_pinned && filter_matches(&filter, &tab.label) {
                                        sleeping_row(ui, index, tab, actions);
                                    }
                                }
                            }
                            downloads_section(ui);
                            bookmark_list(ui, model, &filter, actions);
                        });

                    ui.add_space(6.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        let mut experimental = model.experimental;
                        if look.shows("experimental") && ui
                            .add_sized(
                                [26.0, 26.0],
                                Button::new(RichText::new("☢").size(16.0))
                                    .frame(false)
                                    .selected(experimental),
                            )
                            .on_hover_text("Enable experimental prefs")
                            .clicked()
                        {
                            experimental = !experimental;
                            *location_dirty = false;
                            actions.push(Action::SetExperimental(experimental));
                        }
                        if look.shows("newwindow") && window_button(ui)
                            .on_hover_text("New window")
                            .clicked()
                        {
                            actions.push(Action::NewWindow);
                        }
                        if clock_button(ui)
                            .on_hover_text("History (Ctrl+H)")
                            .clicked()
                        {
                            actions.push(Action::History);
                        }
                        if cog_button(ui).on_hover_text("Settings").clicked() {
                            actions.push(Action::ToggleSettings);
                        }
                    });
                });
        }
    }
}

/// The address bar: a lock (or warning) at the left, the field, a copy-link button at the right;
/// suggestions while typing, Ctrl+L to focus, Enter to go. Shared by the sidebar and the toolbar.
#[allow(clippy::too_many_arguments)]
pub(crate) fn address_bar(
    ui: &mut egui::Ui,
    state: &mut State,
    model: &Model,
    look: &crate::look::Look,
    location: &mut String,
    location_dirty: &mut bool,
    inner_width: f32,
    height: f32,
    actions: &mut Vec<Action>,
) -> egui::Response {
    let location_id = Id::new(LOCATION_ID);
    let field = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let copy = look.shows("copy");
            let mut width = inner_width - 4.0;
            match model.security {
                Security::Secure => {
                    ui.label(RichText::new("\u{1F512}").size(12.0).color(ui.visuals().weak_text_color()))
                        .on_hover_text("Secure connection (https)");
                    width -= 22.0;
                },
                Security::NotSecure => {
                    ui.label(RichText::new("\u{26A0}").size(14.0).color(Color32::from_rgb(220, 150, 40)))
                        .on_hover_text("Not secure (http)");
                    width -= 22.0;
                },
                Security::None => {},
            }
            let field = ui.add_sized(
                [width, height],
                TextEdit::singleline(location)
                    .id(location_id)
                    .vertical_align(egui::Align::Center)
                    .margin(egui::Margin { left: 12, right: if copy { 32 } else { 12 }, top: 0, bottom: 0 })
                    .hint_text("Search or enter address"),
            );
            let copied_id = Id::new("pteron_link_copied");
            let now = ui.input(|input| input.time);
            let copied_at: Option<f64> = ui.memory(|m| m.data.get_temp(copied_id));
            let recently = copied_at.is_some_and(|t| now - t < 1.3);
            let label = if recently { "\u{2713}" } else { "\u{1F5D0}" };
            let copy_rect = Rect::from_center_size(pos2(field.rect.right() - 16.0, field.rect.center().y), vec2(24.0, height));
            if copy && ui
                .put(copy_rect, Button::new(RichText::new(label).size(13.0).color(ui.visuals().weak_text_color())).frame(false))
                .on_hover_text("Copy link")
                .clicked()
            {
                ui.ctx().copy_text(location.clone());
                ui.memory_mut(|m| m.data.insert_temp(copied_id, now));
            }
            if recently {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(400));
            }
            field
        })
        .inner;
    if field.changed() {
        *location_dirty = true;
    }
    if field.has_focus() && *location_dirty && !model.suggestions.is_empty() {
        suggestion_popup(
            ui.ctx(),
            field.rect,
            &model.suggestions,
            location_dirty,
            actions,
        );
    }
    let shortcut = ui.input_mut(|input| {
        input.consume_key(Modifiers::COMMAND, Key::L) ||
            (!cfg!(target_os = "macos") &&
                input.consume_key(Modifiers::ALT, Key::D))
    });
    if shortcut || state.focus_location {
        field.request_focus();
        state.focus_location = false;
    }
    if field.gained_focus() &&
        let Some(mut edit) = TextEditState::load(ui.ctx(), location_id)
    {
        edit.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::new(0),
            CCursor::new(location.len()),
        )));
        edit.store(ui.ctx(), location_id);
    }
    if field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
        actions.push(Action::Go(location.clone()));
    }
    field
}

/// Back, forward, reload and home, as the settings ask. Shared by the sidebar and the toolbar.
pub(crate) fn nav_buttons(
    ui: &mut egui::Ui,
    model: &Model,
    look: &crate::look::Look,
    location_dirty: &mut bool,
    actions: &mut Vec<Action>,
) {
    if look.shows("back") &&
        ui.add_enabled(model.can_go_back, icon_button("⏴")).on_hover_text("Back").clicked()
    {
        *location_dirty = false;
        actions.push(Action::Back);
    }
    if look.shows("forward") &&
        ui.add_enabled(model.can_go_forward, icon_button("⏵")).on_hover_text("Forward").clicked()
    {
        *location_dirty = false;
        actions.push(Action::Forward);
    }
    if look.shows("reload") {
        if model.loading {
            if ui.add(icon_button("✕")).on_hover_text("Stop loading").clicked() {
                actions.push(Action::Stop);
            }
        } else if ui.add(icon_button("↻")).on_hover_text("Reload").clicked() {
            *location_dirty = false;
            actions.push(Action::Reload);
        }
    }
    if look.shows("home") && ui.add(icon_button("⌂")).on_hover_text("New tab page").clicked() {
        *location_dirty = false;
        actions.push(Action::Home);
    }
}

/// The star, reading mode and dark mode for the site.
pub(crate) fn page_buttons(ui: &mut egui::Ui, model: &Model, look: &crate::look::Look, actions: &mut Vec<Action>) {
    if look.shows("bookmark") &&
        star_button(ui, model.bookmarked, model.can_bookmark)
            .on_hover_text(if model.bookmarked { "Remove bookmark" } else { "Bookmark this page" })
            .clicked()
    {
        actions.push(Action::ToggleBookmark);
    }
    if look.shows("reader") &&
        ui.add_enabled(model.can_bookmark, icon_button("Aa"))
            .on_hover_text("Reading mode: clean text and pictures only")
            .clicked()
    {
        actions.push(Action::ReaderMode);
    }
    if let Some(dark) = model.dark_here &&
        look.shows("dark") &&
        ui.add(icon_button(if dark { "◐" } else { "○" }))
            .on_hover_text(if dark { "Dark mode is on for this site (click to turn it off here)" } else { "Dark mode is off for this site (click to turn it on)" })
            .clicked()
    {
        actions.push(Action::ToggleDarkSite);
    }
}

/// What Plume Music is playing, with previous, play/pause and next.
pub(crate) fn music_card(ui: &mut egui::Ui, inner_width: f32) {
    if let Some(now) = crate::music::now_playing() {
        let accent = accent_color();
        ui.add_space(8.0);
        Frame::new()
            .fill(tinted(ui.visuals().window_fill, accent, 0.12))
            .stroke(Stroke::new(1.0_f32, accent.gamma_multiply(0.35)))
            .corner_radius(12.0)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.set_width(inner_width - 22.0);
                ui.horizontal(|ui| {
                    equalizer_icon(ui, accent, now.playing);
                    ui.add(
                        egui::Label::new(RichText::new(&now.title).strong().size(13.0))
                            .truncate(),
                    );
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    // Centre the three buttons.
                    let used = 28.0 + 34.0 + 28.0 + 2.0 * ui.spacing().item_spacing.x * 1.0;
                    ui.add_space(((ui.available_width() - used) / 2.0).max(0.0));
                    if media_button(ui, Glyph::Previous, false, accent)
                        .on_hover_text("Previous song")
                        .clicked()
                    {
                        crate::music::send_command("prev");
                    }
                    let glyph = if now.playing { Glyph::Pause } else { Glyph::Play };
                    if media_button(ui, glyph, true, accent)
                        .on_hover_text(if now.playing { "Pause" } else { "Play" })
                        .clicked()
                    {
                        crate::music::send_command("toggle");
                    }
                    if media_button(ui, Glyph::Next, false, accent)
                        .on_hover_text("Next song")
                        .clicked()
                    {
                        crate::music::send_command("next");
                    }
                });
            });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(1000));
    }
}

/// The bookmarks: loose ones first, then folders (all flat while `filter` is in use).
pub(crate) fn bookmark_list(ui: &mut egui::Ui, model: &Model, filter: &str, actions: &mut Vec<Action>) {
    let shown = model
        .bookmarks
        .iter()
        .filter(|b| filter_matches(filter, &format!("{} {}", b.label, b.url)))
        .count();
    if model.bookmarks.is_empty() || shown == 0 {
        return;
    }
    ui.add_space(6.0);
    ui.separator();
    ui.label(RichText::new("Bookmarks").small().weak());
    let mut folders: Vec<&str> = model
        .bookmarks
        .iter()
        .filter(|b| !b.folder.is_empty())
        .map(|b| b.folder.as_str())
        .collect();
    folders.sort_by_key(|name| name.to_lowercase());
    folders.dedup();
    if !filter.is_empty() {
        // While filtering, matching bookmarks are listed flat.
        for (index, bookmark) in model.bookmarks.iter().enumerate() {
            if filter_matches(filter, &format!("{} {}", bookmark.label, bookmark.url)) {
                bookmark_row(ui, index, bookmark, &folders, false, actions);
            }
        }
        return;
    }
    for (index, bookmark) in model.bookmarks.iter().enumerate() {
        if bookmark.folder.is_empty() {
            bookmark_row(ui, index, bookmark, &folders, true, actions);
        }
    }
    for name in &folders {
        egui::CollapsingHeader::new(RichText::new(*name).size(13.0))
            .id_salt(("pteron_bookmark_folder", *name))
            .default_open(false)
            .show(ui, |ui| {
                for (index, bookmark) in model.bookmarks.iter().enumerate() {
                    if bookmark.folder == *name {
                        bookmark_row(ui, index, bookmark, &folders, true, actions);
                    }
                }
            });
    }
}

/// The list of downloads (hidden when there are none).
pub(crate) fn downloads_section(ui: &mut egui::Ui) {
    use crate::downloads::{self, State};
    let items = downloads::snapshot();
    if items.is_empty() {
        return;
    }
    ui.add_space(6.0);
    ui.separator();
    ui.label(RichText::new("Downloads").small().weak());
    for item in items.iter().rev().take(8) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut name = item.name.clone();
            if name.chars().count() > 26 {
                name = format!("{}...", name.chars().take(25).collect::<String>());
            }
            ui.label(RichText::new(name).strong());
            match &item.state {
                State::Confirm => {
                    ui.label(
                        RichText::new("\u{26A0} This is a program. It can harm your computer if you do not trust the site.")
                            .small()
                            .color(Color32::from_rgb(220, 150, 40)),
                    );
                    ui.horizontal(|ui| {
                        if ui.small_button("Download anyway").clicked() {
                            downloads::confirm(item.id);
                        }
                        if ui.small_button("Cancel").clicked() {
                            downloads::cancel(item.id);
                        }
                    });
                },
                State::Running => {
                    let fraction = item
                        .total
                        .filter(|total| *total > 0)
                        .map(|total| (item.received as f32 / total as f32).clamp(0.0, 1.0));
                    match fraction {
                        Some(fraction) => {
                            ui.add(egui::ProgressBar::new(fraction).desired_height(8.0));
                            ui.label(
                                RichText::new(format!(
                                    "{} of {}",
                                    downloads::format_bytes(item.received),
                                    downloads::format_bytes(item.total.unwrap_or(0))
                                ))
                                .small()
                                .weak(),
                            );
                        },
                        None => {
                            ui.label(
                                RichText::new(format!(
                                    "{} downloaded",
                                    downloads::format_bytes(item.received)
                                ))
                                .small()
                                .weak(),
                            );
                        },
                    }
                    if ui.small_button("Cancel").clicked() {
                        downloads::cancel(item.id);
                    }
                },
                State::Converting(percent) => {
                    ui.add(egui::ProgressBar::new(*percent as f32 / 100.0).desired_height(8.0));
                    ui.label(RichText::new(format!("Making the MP3... {percent}%")).small().weak());
                    if ui.small_button("Cancel").clicked() {
                        downloads::cancel(item.id);
                    }
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
                },
                State::Done => {
                    ui.label(
                        RichText::new(format!("Done, {}", downloads::format_bytes(item.received)))
                            .small()
                            .color(Color32::from_rgb(70, 160, 90)),
                    );
                    ui.horizontal(|ui| {
                        if !item.dangerous && ui.small_button("Open").clicked() {
                            downloads::open_file(item);
                        }
                        if ui.small_button("Show in folder").clicked() {
                            downloads::show_in_folder(&item.path);
                        }
                        if ui.small_button("\u{2212}").on_hover_text("Remove from list").clicked() {
                            downloads::remove(item.id);
                        }
                    });
                },
                State::Failed(reason) => {
                    ui.label(RichText::new(reason.clone()).small().color(Color32::from_rgb(220, 90, 80)));
                    if ui.small_button("Remove").clicked() {
                        downloads::remove(item.id);
                    }
                },
                State::Cancelled => {
                    ui.label(RichText::new("Cancelled").small().weak());
                    if ui.small_button("Remove").clicked() {
                        downloads::remove(item.id);
                    }
                },
            }
        });
    }
}

/// Drag to reorder: remembers which row was picked up, draws a line where it would land, and
/// returns `(from, to)` when it is dropped on this row.
pub(crate) fn drag_to_reorder(ui: &egui::Ui, key: &str, index: usize, rect: Rect, response: &egui::Response) -> Option<(usize, usize)> {
    let key = Id::new(key);
    if response.drag_started() {
        ui.memory_mut(|memory| memory.data.insert_temp(key, index));
    }
    let from: usize = ui.memory(|memory| memory.data.get_temp(key))?;
    if from == index ||
        !ui.rect_contains_pointer(rect) ||
        !ui.input(|input| input.pointer.is_decidedly_dragging() || input.pointer.any_released())
    {
        return None;
    }
    let y = if from > index { rect.min.y } else { rect.max.y };
    ui.painter().line_segment(
        [pos2(rect.min.x + 4.0, y), pos2(rect.max.x - 4.0, y)],
        Stroke::new(2.0_f32, accent_color()),
    );
    ui.input(|input| input.pointer.any_released()).then_some((from, index))
}

fn tab_row(ui: &mut egui::Ui, index: usize, tab: &TabRow, model: &Model, actions: &mut Vec<Action>) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), row_height()), Sense::click_and_drag());
    if let Some((from, to)) = drag_to_reorder(ui, "pteron_tab_drag", index, rect, &response) {
        actions.push(Action::MoveTab { from, to });
    }
    let close_rect = Rect::from_min_size(
        pos2(rect.max.x - 26.0, rect.min.y + 2.0),
        vec2(24.0, row_height() - 4.0),
    );
    let close = ui.interact(close_rect, response.id.with("close"), Sense::click());
    let audio_click = (tab.audible || tab.muted) &&
        ui.interact(audio_rect(pos2(rect.max.x - 30.0, rect.center().y)), response.id.with("audio"), Sense::click())
            .on_hover_text(if tab.muted { "Unmute tab" } else { "Mute tab" })
            .clicked();
    let row_hovered = ui.rect_contains_pointer(rect);

    let visuals = ui.visuals();
    let fill = if tab.active {
        Some(visuals.widgets.active.weak_bg_fill)
    } else if row_hovered {
        Some(visuals.widgets.hovered.weak_bg_fill)
    } else {
        None
    };
    let text_color = visuals.text_color();
    let painter = ui.painter();
    if let Some(fill) = fill {
        painter.rect_filled(rect, 8.0, fill);
    }
    if tab.active {
        // A bar in the accent colour on the left edge marks the tab you are on.
        painter.rect_filled(
            Rect::from_min_size(pos2(rect.min.x + 1.0, rect.min.y + 6.0), vec2(3.0, rect.height() - 12.0)),
            2.0,
            accent_color(),
        );
    }
    if let Some(favicon) = tab.favicon {
        let icon_rect = Rect::from_min_size(pos2(rect.min.x + 6.0, rect.center().y - 8.0), vec2(16.0, 16.0));
        painter.image(
            favicon.id,
            icon_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    let marks = tab_marks(painter, pos2(rect.max.x - 30.0, rect.center().y), tab, text_color);
    let text_clip = Rect::from_min_max(rect.min, pos2(rect.max.x - 28.0 - marks, rect.max.y));
    painter.with_clip_rect(text_clip).text(
        pos2(rect.min.x + 30.0, rect.center().y),
        Align2::LEFT_CENTER,
        &tab.label,
        FontId::proportional(13.0),
        text_color,
    );
    if tab.pinned {
        // A pinned tab shows a pin instead of a close button; clicking it unpins.
        pin_glyph(painter, close_rect.center(), accent_color(), row_hovered);
    } else if row_hovered || close_always() {
        if close.hovered() {
            painter.circle_filled(close_rect.center(), 10.0, text_color.gamma_multiply(0.18));
        }
        painter.text(
            close_rect.center(),
            Align2::CENTER_CENTER,
            "×",
            FontId::proportional(16.0),
            text_color,
        );
    }

    if audio_click {
        actions.push(Action::ToggleMute(index));
    } else if tab.pinned {
        if close.clicked() {
            actions.push(Action::TogglePin(index));
        } else if response.clicked() && !tab.active {
            actions.push(Action::Activate(index));
        }
    } else if close.clicked() || close.middle_clicked() || response.middle_clicked() {
        actions.push(Action::Close(index));
    } else if response.clicked() && !tab.active {
        actions.push(Action::Activate(index));
    }
    let response = if tab.label.chars().count() > 26 { response.on_hover_text(&tab.label) } else { response };
    response.context_menu(|ui| tab_menu(ui, index, model, actions));
}

/// Small signs on a tab, drawn leftwards from `right`: an eye for "kept awake", two panes for
/// split view. Returns the width they took.
pub(crate) fn tab_marks(painter: &egui::Painter, right: Pos2, tab: &TabRow, color: Color32) -> f32 {
    let stroke = Stroke::new(1.2_f32, color.gamma_multiply(0.75));
    let mut used = 0.0;
    if tab.private {
        painter.circle_filled(right - vec2(used + 7.0, 0.0), 4.0, PRIVATE);
        used += 16.0;
    }
    if tab.audible || tab.muted {
        speaker_glyph(painter, audio_rect(right).center(), tab.muted, color);
        used += 18.0;
    }
    if tab.awake {
        let center = right - vec2(used + 7.0, 0.0);
        painter.circle_stroke(center, 4.5, stroke);
        painter.circle_filled(center, 1.6, stroke.color);
        used += 16.0;
    }
    if tab.split {
        let center = right - vec2(used + 7.0, 0.0);
        painter.rect_stroke(Rect::from_center_size(center, vec2(12.0, 9.0)), 1.5, stroke, egui::StrokeKind::Middle);
        painter.line_segment([center - vec2(0.0, 4.5), center + vec2(0.0, 4.5)], stroke);
        used += 16.0;
    }
    used
}

/// Where the speaker of a tab that plays sound is (the rightmost mark): click it to mute.
pub(crate) fn audio_rect(right: Pos2) -> Rect {
    Rect::from_center_size(right - vec2(8.0, 0.0), vec2(18.0, 18.0))
}

/// A small speaker, with sound waves, or crossed out when muted.
pub(crate) fn speaker_glyph(painter: &egui::Painter, center: Pos2, muted: bool, color: Color32) {
    let stroke = Stroke::new(1.3_f32, color);
    let c = center - vec2(3.0, 0.0);
    painter.add(egui::Shape::convex_polygon(
        vec![c + vec2(-4.0, -2.0), c + vec2(-1.5, -2.0), c + vec2(2.0, -5.0), c + vec2(2.0, 5.0), c + vec2(-1.5, 2.0), c + vec2(-4.0, 2.0)],
        color,
        Stroke::NONE,
    ));
    if muted {
        painter.line_segment([c + vec2(4.5, -2.5), c + vec2(9.5, 2.5)], stroke);
        painter.line_segment([c + vec2(4.5, 2.5), c + vec2(9.5, -2.5)], stroke);
    } else {
        for (radius, steps) in [(4.5_f32, 6), (8.0, 8)] {
            let points: Vec<Pos2> = (0..=steps)
                .map(|i| {
                    let angle = -0.9 + 1.8 * i as f32 / steps as f32;
                    c + vec2(1.5 + radius * angle.cos(), radius * angle.sin())
                })
                .collect();
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}

/// The right-click menu of a live tab.
pub(crate) fn tab_menu(ui: &mut egui::Ui, index: usize, model: &Model, actions: &mut Vec<Action>) {
    fn item(ui: &mut egui::Ui, text: &str, action: Action, actions: &mut Vec<Action>) {
        if ui.button(text).clicked() {
            actions.push(action);
            ui.close();
        }
    }
    let Some(tab) = model.tabs.get(index) else {
        return;
    };
    item(ui, "Reload", Action::ReloadTab(index), actions);
    item(ui, "Duplicate", Action::Duplicate(index), actions);
    item(ui, if tab.muted { "Unmute tab" } else { "Mute tab" }, Action::ToggleMute(index), actions);
    item(ui, "Take screenshot", Action::Screenshot(index), actions);
    ui.separator();
    item(ui, if tab.pinned { "Unpin tab" } else { "Pin tab" }, Action::TogglePin(index), actions);
    let mut awake = tab.awake;
    if ui
        .checkbox(&mut awake, "Keep awake")
        .on_hover_text("This tab never goes to sleep, until you close it or Sirocco")
        .changed()
    {
        actions.push(Action::ToggleAwake(index));
        ui.close();
    }
    if tab.split {
        item(ui, "Leave split view", Action::LeaveSplit, actions);
    } else if !tab.active {
        item(ui, "Show beside the current tab", Action::SplitWith(index), actions);
    } else if model.tabs.len() > 1 {
        ui.menu_button("Split view with", |ui| {
            for (other, row) in model.tabs.iter().enumerate().filter(|(other, _)| *other != index) {
                let label: String = row.label.chars().take(40).collect();
                item(ui, &label, Action::SplitWith(other), actions);
            }
        });
    }
    ui.separator();
    item(ui, "Open in default browser", Action::OpenExternal(index), actions);
    if ui.button("Check this page").on_hover_text("Find out why a page is blank or broken").clicked() {
        actions.push(Action::CheckPage(index));
        ui.close();
    }
    ui.separator();
    if ui.button("Close tab").clicked() {
        if tab.pinned {
            actions.push(Action::TogglePin(index));
        }
        actions.push(Action::Close(index));
        ui.close();
    }
    if model.tabs.len() + model.sleeping.len() > 1 {
        item(ui, "Close other tabs", Action::CloseOthers(index), actions);
        item(ui, "Close tabs after this one", Action::CloseAfter(index), actions);
    }
}

/// The list of matching pages under the address bar. Rows react to the mouse *press*, because
/// the address bar loses focus (and this list disappears) before the button is released.
pub(crate) fn suggestion_popup(
    ctx: &Context,
    field: Rect,
    suggestions: &[crate::library::Suggestion],
    location_dirty: &mut bool,
    actions: &mut Vec<Action>,
) {
    Area::new(Id::new("pteron_suggestions"))
        .order(Order::Tooltip)
        .fixed_pos(field.left_bottom() + vec2(0.0, 2.0))
        .show(ctx, |ui| {
            Frame::popup(ui.style()).inner_margin(4.0).show(ui, |ui| {
                ui.set_width(field.width() - 8.0);
                for suggestion in suggestions {
                    let (rect, response) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                    let visuals = ui.visuals();
                    if response.hovered() {
                        ui.painter()
                            .rect_filled(rect, 5.0, visuals.widgets.hovered.weak_bg_fill);
                    }
                    let text_color = visuals.text_color();
                    let weak = visuals.weak_text_color();
                    let title = if suggestion.title.is_empty() {
                        suggestion.url.as_str()
                    } else {
                        suggestion.title.as_str()
                    };
                    let clip = ui.painter().with_clip_rect(rect.shrink2(vec2(4.0, 0.0)));
                    // A star for bookmarks, a clock for pages from history.
                    clip.text(
                        pos2(rect.min.x + 14.0, rect.center().y),
                        Align2::CENTER_CENTER,
                        if suggestion.bookmarked { "\u{2605}" } else { "\u{1F558}" },
                        FontId::proportional(14.0),
                        if suggestion.bookmarked { accent_color() } else { weak },
                    );
                    clip.text(
                        pos2(rect.min.x + 30.0, rect.min.y + 11.0),
                        Align2::LEFT_CENTER,
                        title,
                        FontId::proportional(13.0),
                        text_color,
                    );
                    clip.text(
                        pos2(rect.min.x + 30.0, rect.min.y + 27.0),
                        Align2::LEFT_CENTER,
                        &suggestion.url,
                        FontId::proportional(11.0),
                        weak,
                    );
                    let pressed = ui.input(|input| input.pointer.primary_pressed());
                    if pressed && response.hovered() {
                        *location_dirty = false;
                        actions.push(Action::Go(suggestion.url.clone()));
                    }
                }
            });
        });
}

/// A bookmark: click opens it here, middle-click opens it in a new tab.
pub(crate) fn bookmark_row(
    ui: &mut egui::Ui,
    index: usize,
    bookmark: &BookmarkRow,
    folders: &[&str],
    can_drag: bool,
    actions: &mut Vec<Action>,
) {
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), row_height()),
        if can_drag { Sense::click_and_drag() } else { Sense::click() },
    );
    if can_drag &&
        let Some((from, to)) = drag_to_reorder(ui, "pteron_bookmark_drag", index, rect, &response)
    {
        actions.push(Action::MoveBookmarkTo { from, to });
    }
    let visuals = ui.visuals();
    let fill = response
        .hovered()
        .then_some(visuals.widgets.hovered.weak_bg_fill);
    let text_color = visuals.text_color();
    let painter = ui.painter();
    if let Some(fill) = fill {
        painter.rect_filled(rect, 8.0, fill);
    }
    painter.with_clip_rect(rect).text(
        pos2(rect.min.x + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        &bookmark.label,
        FontId::proportional(13.0),
        text_color,
    );
    if response.middle_clicked() {
        actions.push(Action::OpenBookmarkInNewTab(index));
    } else if response.clicked() {
        actions.push(Action::OpenBookmark(index));
    }
    let response = response.on_hover_text(&bookmark.url);
    response.context_menu(|ui| {
        if ui.button("Open in new tab").clicked() {
            actions.push(Action::OpenBookmarkInNewTab(index));
            ui.close();
        }
        ui.menu_button("Move to folder", |ui| {
            if !bookmark.folder.is_empty() && ui.button("No folder").clicked() {
                actions.push(Action::MoveBookmark(index, String::new()));
                ui.close();
            }
            for name in folders {
                if *name != bookmark.folder && ui.button(*name).clicked() {
                    actions.push(Action::MoveBookmark(index, name.to_string()));
                    ui.close();
                }
            }
            ui.separator();
            ui.label(RichText::new("New folder").small().weak());
            let id = Id::new(("pteron_new_folder", index));
            let mut text = ui.memory_mut(|m| m.data.get_temp::<String>(id).unwrap_or_default());
            let edit = ui.add(egui::TextEdit::singleline(&mut text).desired_width(140.0));
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if enter && !text.trim().is_empty() {
                actions.push(Action::MoveBookmark(index, text.trim().to_string()));
                text.clear();
                ui.close();
            }
            ui.memory_mut(|m| m.data.insert_temp(id, text));
        });
        if ui.button("Remove bookmark").clicked() {
            actions.push(Action::RemoveBookmark(index));
            ui.close();
        }
    });
}

/// A small pin, drawn with the painter. Filled when `strong`.
pub(crate) fn pin_glyph(painter: &egui::Painter, center: Pos2, color: Color32, strong: bool) {
    let color = if strong { color } else { color.gamma_multiply(0.6) };
    painter.circle_filled(center + vec2(0.0, -3.0), 3.2, color);
    painter.line_segment(
        [center + vec2(0.0, -1.0), center + vec2(0.0, 6.0)],
        Stroke::new(1.4_f32, color),
    );
}

fn sleeping_row(ui: &mut egui::Ui, index: usize, tab: &SleepingRow, actions: &mut Vec<Action>) {
    let label = tab.label.as_str();
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), row_height()), Sense::click());
    let visuals = ui.visuals();
    let fill = response
        .hovered()
        .then_some(visuals.widgets.hovered.weak_bg_fill);
    let text_color = visuals.weak_text_color();
    let painter = ui.painter();
    if let Some(fill) = fill {
        painter.rect_filled(rect, 6.0, fill);
    }
    painter.with_clip_rect(rect).text(
        pos2(rect.min.x + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        format!("zZ  {label}"),
        FontId::proportional(13.0),
        text_color,
    );
    if response.middle_clicked() {
        actions.push(Action::CloseSleeping(index));
    } else if response.clicked() {
        actions.push(Action::Wake(index));
    }
    if tab.pinned {
        pin_glyph(
            painter,
            pos2(rect.max.x - 14.0, rect.center().y),
            text_color,
            false,
        );
    }
    response.context_menu(|ui| sleeping_menu(ui, index, tab.pinned, actions));
}

/// The right-click menu of a sleeping tab.
pub(crate) fn sleeping_menu(ui: &mut egui::Ui, index: usize, pinned: bool, actions: &mut Vec<Action>) {
    if ui.button(if pinned { "Unpin tab" } else { "Pin tab" }).clicked() {
        actions.push(Action::TogglePinSleeping(index));
        ui.close();
    }
    if ui.button("Close tab").clicked() {
        actions.push(Action::CloseSleeping(index));
        ui.close();
    }
}
