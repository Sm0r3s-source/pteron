/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pteron: top-tab layouts in the style of other browsers (Chrome, Firefox, Opera, Opera GX): a
//! strip of tabs, a toolbar with the address bar and a menu, an optional bookmarks bar and, for
//! the Opera styles, a bar of shortcuts down the left side. It uses the sidebar's model and
//! actions, so `gui.rs` handles both layouts the same way.

use egui::{
    Align2, Color32, CornerRadius, FontId, Frame, Id, Margin, Panel, Popup, Pos2, Rect, RichText,
    ScrollArea, Sense, Stroke, StrokeKind, pos2, vec2,
};

use super::sidebar::{self, Action, Model, State, accent_color, tinted};
use crate::look::Look;

#[derive(Clone, Copy, PartialEq)]
enum Style {
    Chrome,
    Firefox,
    Opera,
    Gx,
}

impl Style {
    fn of(look: &Look) -> Self {
        match look.layout.as_str() {
            "firefox" => Style::Firefox,
            "opera" => Style::Opera,
            "gx" => Style::Gx,
            _ => Style::Chrome,
        }
    }
}

/// The panel opened from the Opera-style side bar.
#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) enum RailPanel {
    #[default]
    Closed,
    Bookmarks,
    Downloads,
    Music,
}

/// Colours of the tab strip, toolbar and address field, from the theme.
struct Colors {
    strip: Color32,
    bar: Color32,
    field: Color32,
    text: Color32,
    weak: Color32,
    line: Color32,
    hover: Color32,
    accent: Color32,
}

fn colors(ui: &egui::Ui, style: Style) -> Colors {
    let v = ui.visuals();
    let text = v.text_color();
    let lum = |c: Color32| (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0;
    // The plain looks have almost the same panel and card colours: darken the tab strip.
    let (strip, bar) = if (lum(v.panel_fill) - lum(v.faint_bg_color)).abs() < 0.03 {
        let strip = if v.dark_mode {
            tinted(v.panel_fill, Color32::BLACK, 0.35)
        } else {
            tinted(v.panel_fill, text, 0.08)
        };
        (strip, v.panel_fill)
    } else {
        (v.panel_fill, v.faint_bg_color)
    };
    let field = match style {
        Style::Gx => tinted(bar, Color32::BLACK, 0.4),
        _ => tinted(bar, text, if v.dark_mode { 0.09 } else { 0.06 }),
    };
    Colors {
        strip,
        bar,
        field,
        text,
        weak: v.weak_text_color(),
        line: tinted(bar, text, 0.14),
        hover: tinted(strip, text, 0.08),
        accent: accent_color(),
    }
}

/// Draw the layout and report what was clicked.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &mut State,
    rail: &mut RailPanel,
    model: &Model,
    look: &Look,
    location: &mut String,
    location_dirty: &mut bool,
) -> Vec<Action> {
    let mut actions = Vec::new();
    if ui.input(|input| !input.pointer.any_down() && !input.pointer.any_released()) {
        ui.memory_mut(|memory| memory.data.remove::<usize>(Id::new(DRAG_KEY)));
    }
    let style = Style::of(look);
    let c = colors(ui, style);
    let row = look.row_height();
    // Opera GX's side bar runs the full height of the window; Opera's starts under the toolbar.
    if style == Style::Gx {
        side_rail(ui, style, &c, rail, &mut actions);
    }
    tab_strip(ui, style, &c, model, look, row, &mut actions);
    let with_bar = look.bookmarks_bar && !model.bookmarks.is_empty();
    toolbar(ui, style, &c, state, model, look, location, location_dirty, row, !with_bar, &mut actions);
    if with_bar {
        bookmarks_bar(ui, &c, model, row, &mut actions);
    }
    if style == Style::Opera {
        side_rail(ui, style, &c, rail, &mut actions);
    }
    if matches!(style, Style::Opera | Style::Gx) && *rail != RailPanel::Closed {
        rail_panel(ui, &c, rail, model, &mut actions);
    }
    actions
}

const DRAG_KEY: &str = "pteron_top_tab_drag";

/// One tab in the strip; returns its response and whether its close button was clicked.
#[allow(clippy::too_many_arguments)]
fn tab(
    ui: &mut egui::Ui,
    style: Style,
    c: &Colors,
    look: &Look,
    rect: Rect,
    id: Id,
    label: &str,
    favicon: Option<egui::load::SizedTexture>,
    active: bool,
    sleeping: bool,
    pinned: bool,
) -> (egui::Response, bool) {
    let response = ui.interact(rect, id, if sleeping { Sense::click() } else { Sense::click_and_drag() });
    let hovered = response.hovered();
    let r = look.roundness as u8;
    let painter = ui.painter().clone();
    let shape = match style {
        Style::Chrome => Rect::from_min_max(pos2(rect.min.x, rect.min.y + 6.0), rect.max),
        Style::Gx => Rect::from_min_max(pos2(rect.min.x + 1.0, rect.min.y + 5.0), rect.max),
        Style::Firefox | Style::Opera => rect.shrink2(vec2(2.0, 5.0)),
    };
    if active {
        let corners = match style {
            Style::Chrome | Style::Gx => CornerRadius { nw: r + 2, ne: r + 2, sw: 0, se: 0 },
            _ => CornerRadius::same(r + 2),
        };
        if style == Style::Firefox {
            painter.rect_filled(shape.translate(vec2(0.0, 1.0)), corners, Color32::from_black_alpha(28));
        }
        painter.rect_filled(shape, corners, c.bar);
        if style == Style::Gx {
            painter.rect_filled(
                Rect::from_min_max(pos2(shape.min.x, shape.max.y - 2.0), shape.max),
                0.0,
                c.accent,
            );
        }
    } else if hovered {
        let fill = if style == Style::Gx { tinted(c.strip, c.accent, 0.14) } else { c.hover };
        painter.rect_filled(shape.shrink2(vec2(1.0, 2.0)), r + 2, fill);
    } else if matches!(style, Style::Chrome | Style::Gx) && !pinned {
        // A thin line between tabs that are not in use.
        let x = rect.max.x - 0.5;
        painter.line_segment(
            [pos2(x, shape.center().y - 8.0), pos2(x, shape.center().y + 8.0)],
            Stroke::new(1.0_f32, if style == Style::Gx { c.accent.gamma_multiply(0.35) } else { c.line }),
        );
    }
    let center_y = shape.center().y;
    let icon = favicon.filter(|_| look.tab_icons);
    if pinned {
        let center = pos2(shape.center().x, center_y);
        match icon {
            Some(icon) => {
                let tint = if sleeping { Color32::from_white_alpha(120) } else { Color32::WHITE };
                painter.image(icon.id, Rect::from_center_size(center, vec2(16.0, 16.0)), UV, tint);
            },
            None => {
                let letter = label.chars().find(|c| c.is_alphanumeric()).unwrap_or('•');
                let color = if sleeping { c.weak } else { c.text };
                painter.text(center, Align2::CENTER_CENTER, letter.to_uppercase(), FontId::proportional(14.0), color);
            },
        }
        return (response, false);
    }
    let inner = shape.shrink2(vec2(10.0, 0.0));
    let mut text_x = inner.min.x;
    if let Some(icon) = icon {
        painter.image(icon.id, Rect::from_center_size(pos2(text_x + 8.0, center_y), vec2(16.0, 16.0)), UV, Color32::WHITE);
        text_x += 24.0;
    }
    let show_close = !sleeping && rect.width() > 72.0 && (look.close_always || hovered || active);
    let close_rect = Rect::from_center_size(pos2(inner.max.x - 8.0, center_y), vec2(20.0, 20.0));
    let text_right = if show_close { close_rect.min.x - 4.0 } else { inner.max.x };
    let color = if sleeping {
        c.weak
    } else if active {
        c.text
    } else {
        tinted(c.text, c.strip, 0.25)
    };
    let text = if sleeping { format!("zZ  {label}") } else { label.to_string() };
    painter
        .with_clip_rect(Rect::from_min_max(pos2(text_x, shape.min.y), pos2(text_right, shape.max.y)))
        .text(pos2(text_x, center_y), Align2::LEFT_CENTER, text, FontId::proportional(13.0), color);
    let mut closed = false;
    if show_close {
        let close = ui.interact(close_rect, id.with("close"), Sense::click());
        if close.hovered() {
            painter.circle_filled(close_rect.center(), 10.0, c.text.gamma_multiply(0.15));
        }
        painter.text(close_rect.center(), Align2::CENTER_CENTER, "×", FontId::proportional(15.0), color);
        closed = close.clicked();
    }
    (response, closed)
}

const UV: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

fn tab_strip(ui: &mut egui::Ui, style: Style, c: &Colors, model: &Model, look: &Look, row: f32, actions: &mut Vec<Action>) {
    let height = row + 10.0;
    Panel::top("pteron_tabstrip")
        .exact_size(height)
        .resizable(false)
        .frame(Frame::NONE.fill(c.strip))
        .show_inside(ui, |ui| {
            let full = ui.max_rect();
            let pinned = model.tabs.iter().filter(|t| t.pinned).count() + model.sleeping.iter().filter(|t| t.pinned).count();
            let normal = model.tabs.len() + model.sleeping.len() - pinned;
            let end = full.max.x - 40.0;
            let spare = end - full.min.x - 8.0 - 36.0 - pinned as f32 * 44.0;
            let width = if normal == 0 { 0.0 } else { (spare / normal as f32).clamp(56.0, 240.0) };
            let mut x = full.min.x + 8.0;
            let drag_from: Option<usize> = ui.memory(|memory| memory.data.get_temp(Id::new(DRAG_KEY)));
            for section_pinned in [true, false] {
                let w = if section_pinned { 44.0 } else { width };
                for (index, t) in model.tabs.iter().enumerate().filter(|(_, t)| t.pinned == section_pinned) {
                    if x + w > end + 1.0 {
                        break;
                    }
                    let rect = Rect::from_min_size(pos2(x, full.min.y), vec2(w, height));
                    x += w;
                    let (response, closed) =
                        tab(ui, style, c, look, rect, Id::new(("pteron_top_tab", index)), &t.label, t.favicon, t.active, false, t.pinned);
                    if response.drag_started() {
                        ui.memory_mut(|memory| memory.data.insert_temp(Id::new(DRAG_KEY), index));
                    }
                    if let Some(from) = drag_from.filter(|from| *from != index) &&
                        ui.rect_contains_pointer(rect)
                    {
                        let edge = if from > index { rect.min.x } else { rect.max.x };
                        ui.painter().line_segment([pos2(edge, rect.min.y + 6.0), pos2(edge, rect.max.y - 4.0)], Stroke::new(2.0_f32, c.accent));
                        if ui.input(|input| input.pointer.any_released()) {
                            actions.push(Action::MoveTab { from, to: index });
                        }
                    }
                    if closed || response.middle_clicked() {
                        actions.push(if t.pinned { Action::TogglePin(index) } else { Action::Close(index) });
                    } else if response.clicked() && !t.active {
                        actions.push(Action::Activate(index));
                    }
                    let response = response.on_hover_text(&t.label);
                    response.context_menu(|ui| sidebar::tab_menu(ui, index, t.pinned, actions));
                }
                for (index, t) in model.sleeping.iter().enumerate().filter(|(_, t)| t.pinned == section_pinned) {
                    if x + w > end + 1.0 {
                        break;
                    }
                    let rect = Rect::from_min_size(pos2(x, full.min.y), vec2(w, height));
                    x += w;
                    let (response, _) =
                        tab(ui, style, c, look, rect, Id::new(("pteron_top_sleeping", index)), &t.label, None, false, true, t.pinned);
                    if response.middle_clicked() {
                        actions.push(Action::CloseSleeping(index));
                    } else if response.clicked() {
                        actions.push(Action::Wake(index));
                    }
                    let response = response.on_hover_text(format!("Sleeping tab (click to wake)\n{}", t.label));
                    response.context_menu(|ui| sidebar::sleeping_menu(ui, index, t.pinned, actions));
                }
            }
            // New tab, right after the tabs.
            let plus = Rect::from_center_size(pos2(x + 20.0, full.min.y + 5.0 + row / 2.0 + 1.0), vec2(28.0, 28.0));
            if icon_at(ui, plus, Icon::Plus, c, "pteron_top_new").on_hover_text("New tab (Ctrl+T)").clicked() {
                actions.push(Action::NewTab);
            }
            // Every tab in a list, for when they do not all fit.
            let list = Rect::from_center_size(pos2(full.max.x - 22.0, plus.center().y), vec2(28.0, 28.0));
            let response = icon_at(ui, list, Icon::Chevron, c, "pteron_top_list").on_hover_text("List all tabs");
            let _ = Popup::menu(&response).show(|ui| {
                ui.set_min_width(260.0);
                ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
                    for (index, t) in model.tabs.iter().enumerate() {
                        if ui.selectable_label(t.active, short(&t.label, 48)).clicked() {
                            actions.push(Action::Activate(index));
                            ui.close();
                        }
                    }
                    for (index, t) in model.sleeping.iter().enumerate() {
                        if ui.button(RichText::new(format!("zZ  {}", short(&t.label, 44))).weak()).clicked() {
                            actions.push(Action::Wake(index));
                            ui.close();
                        }
                    }
                });
            });
        });
}

fn short(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        format!("{}…", text.chars().take(max - 1).collect::<String>())
    } else {
        text.to_string()
    }
}

#[allow(clippy::too_many_arguments)]
fn toolbar(
    ui: &mut egui::Ui,
    style: Style,
    c: &Colors,
    state: &mut State,
    model: &Model,
    look: &Look,
    location: &mut String,
    location_dirty: &mut bool,
    row: f32,
    last: bool,
    actions: &mut Vec<Action>,
) {
    Panel::top("pteron_toolbar")
        .exact_size(row + 12.0)
        .resizable(false)
        .frame(Frame::NONE.fill(c.bar).inner_margin(Margin::symmetric(8, 0)))
        .show_inside(ui, |ui| {
            let rect = ui.max_rect();
            if last {
                ui.painter().hline(rect.x_range().expand(8.0), rect.max.y - 0.5, Stroke::new(1.0_f32, c.line));
            }
            ui.horizontal_centered(|ui| {
                sidebar::nav_buttons(ui, model, look, location_dirty, actions);
                let downloads = !crate::downloads::snapshot().is_empty();
                let playing = crate::music::now_playing().is_some();
                let page_buttons = ["bookmark", "reader"].iter().filter(|b| look.shows(b)).count() +
                    usize::from(model.dark_here.is_some() && look.shows("dark"));
                let right = 34.0 * (page_buttons + 1 + usize::from(downloads) + usize::from(playing)) as f32;
                let available = (ui.available_width() - right - 12.0).max(120.0);
                // Firefox keeps open space on both sides of the address bar on wide windows.
                let width = if style == Style::Firefox { available.min(available * 0.55 + 300.0) } else { available };
                let lead = (available - width) / 2.0;
                ui.add_space(lead + 4.0);
                ui.scope(|ui| {
                    let v = ui.visuals_mut();
                    v.text_edit_bg_color = Some(c.field);
                    let radius = match style {
                        Style::Chrome => ((row + 2.0) / 2.0) as u8,
                        _ => look.roundness as u8,
                    };
                    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
                        w.corner_radius = CornerRadius::same(radius);
                    }
                    v.widgets.inactive.bg_stroke = if style == Style::Gx {
                        Stroke::new(1.0_f32, c.accent.gamma_multiply(0.45))
                    } else {
                        Stroke::NONE
                    };
                    v.selection.stroke = Stroke::new(2.0_f32, c.accent);
                    sidebar::address_bar(ui, state, model, look, location, location_dirty, width, row + 2.0, actions);
                });
                ui.add_space(lead);
                sidebar::page_buttons(ui, model, look, actions);
                if playing {
                    let response = icon_button(ui, Icon::Music, c, "pteron_top_music").on_hover_text("Music");
                    let _ = Popup::menu(&response).show(|ui| sidebar::music_card(ui, 260.0));
                }
                if downloads {
                    let response = icon_button(ui, Icon::Download, c, "pteron_top_downloads").on_hover_text("Downloads");
                    let _ = Popup::menu(&response).show(|ui| {
                        ui.set_width(280.0);
                        sidebar::downloads_section(ui);
                    });
                }
                let menu_icon = if style == Style::Chrome { Icon::Dots } else { Icon::Lines };
                let response = icon_button(ui, menu_icon, c, "pteron_top_menu").on_hover_text("Menu");
                let _ = Popup::menu(&response).show(|ui| main_menu(ui, model, look, actions));
            });
        });
}

fn main_menu(ui: &mut egui::Ui, model: &Model, look: &Look, actions: &mut Vec<Action>) {
    fn item(ui: &mut egui::Ui, text: &str, action: Action, actions: &mut Vec<Action>) {
        if ui.button(text).clicked() {
            actions.push(action);
            ui.close();
        }
    }
    ui.set_min_width(230.0);
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.spacing_mut().button_padding = vec2(8.0, 5.0);
    item(ui, "New tab", Action::NewTab, actions);
    item(ui, "New window", Action::NewWindow, actions);
    ui.separator();
    ui.menu_button("Bookmarks", |ui| bookmarks_menu(ui, model, actions));
    if !crate::downloads::snapshot().is_empty() {
        ui.menu_button("Downloads", |ui| {
            ui.set_width(280.0);
            sidebar::downloads_section(ui);
        });
    }
    if ui.button("Find in page").clicked() {
        crate::find::open();
        ui.close();
    }
    if model.can_bookmark {
        item(ui, "Reading mode", Action::ReaderMode, actions);
    }
    if let Some(dark) = model.dark_here {
        item(ui, if dark { "Dark mode off for this site" } else { "Dark mode on for this site" }, Action::ToggleDarkSite, actions);
    }
    let mut bar = look.bookmarks_bar;
    if ui.checkbox(&mut bar, "Bookmarks bar").changed() {
        actions.push(Action::ToggleBookmarksBar);
    }
    ui.separator();
    if look.shows("experimental") {
        let mut experimental = model.experimental;
        if ui.checkbox(&mut experimental, "Experimental web features").changed() {
            actions.push(Action::SetExperimental(experimental));
        }
    }
    item(ui, "Settings", Action::ToggleSettings, actions);
}

/// Loose bookmarks, then one submenu per folder.
fn bookmarks_menu(ui: &mut egui::Ui, model: &Model, actions: &mut Vec<Action>) {
    if model.bookmarks.is_empty() {
        ui.label(RichText::new("No bookmarks yet. Click the star to add one.").weak());
        return;
    }
    let open = |ui: &mut egui::Ui, index: usize, bookmark: &sidebar::BookmarkRow, actions: &mut Vec<Action>| {
        let response = ui.button(short(&bookmark.label, 44)).on_hover_text(&bookmark.url);
        if response.middle_clicked() {
            actions.push(Action::OpenBookmarkInNewTab(index));
            ui.close();
        } else if response.clicked() {
            actions.push(Action::OpenBookmark(index));
            ui.close();
        }
    };
    let mut folders: Vec<&str> = model.bookmarks.iter().map(|b| b.folder.as_str()).filter(|f| !f.is_empty()).collect();
    folders.sort_by_key(|name| name.to_lowercase());
    folders.dedup();
    for name in &folders {
        ui.menu_button(*name, |ui| {
            for (index, bookmark) in model.bookmarks.iter().enumerate().filter(|(_, b)| b.folder == *name) {
                open(ui, index, bookmark, actions);
            }
        });
    }
    for (index, bookmark) in model.bookmarks.iter().enumerate().filter(|(_, b)| b.folder.is_empty()) {
        open(ui, index, bookmark, actions);
    }
}

fn bookmarks_bar(ui: &mut egui::Ui, c: &Colors, model: &Model, row: f32, actions: &mut Vec<Action>) {
    Panel::top("pteron_bookmarks_bar")
        .exact_size(row + 2.0)
        .resizable(false)
        .frame(Frame::NONE.fill(c.bar).inner_margin(Margin::symmetric(8, 0)))
        .show_inside(ui, |ui| {
            let rect = ui.max_rect();
            ui.painter().hline(rect.x_range().expand(8.0), rect.max.y - 0.5, Stroke::new(1.0_f32, c.line));
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                // Folders look like the other entries: no box until the mouse is over them.
                let widgets = &mut ui.visuals_mut().widgets;
                widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                widgets.inactive.bg_stroke = Stroke::NONE;
                let mut folders: Vec<&str> = model.bookmarks.iter().map(|b| b.folder.as_str()).filter(|f| !f.is_empty()).collect();
                folders.sort_by_key(|name| name.to_lowercase());
                folders.dedup();
                for name in folders {
                    ui.menu_button(RichText::new(format!("{name} ⏷")).size(12.5).color(c.text), |ui| {
                        for (index, bookmark) in model.bookmarks.iter().enumerate().filter(|(_, b)| b.folder == name) {
                            if ui.button(short(&bookmark.label, 44)).on_hover_text(&bookmark.url).clicked() {
                                actions.push(Action::OpenBookmark(index));
                                ui.close();
                            }
                        }
                    });
                }
                for (index, bookmark) in model.bookmarks.iter().enumerate().filter(|(_, b)| b.folder.is_empty()) {
                    let response = ui
                        .add(egui::Button::new(RichText::new(short(&bookmark.label, 24)).size(12.5).color(c.text)).frame(false))
                        .on_hover_text(&bookmark.url);
                    if response.middle_clicked() {
                        actions.push(Action::OpenBookmarkInNewTab(index));
                    } else if response.clicked() {
                        actions.push(Action::OpenBookmark(index));
                    }
                    response.context_menu(|ui| {
                        if ui.button("Open in new tab").clicked() {
                            actions.push(Action::OpenBookmarkInNewTab(index));
                            ui.close();
                        }
                        if ui.button("Remove bookmark").clicked() {
                            actions.push(Action::RemoveBookmark(index));
                            ui.close();
                        }
                    });
                }
            });
        });
}

/// The Opera-style bar down the left side.
fn side_rail(ui: &mut egui::Ui, style: Style, c: &Colors, rail: &mut RailPanel, actions: &mut Vec<Action>) {
    let fill = if style == Style::Gx { tinted(c.strip, Color32::BLACK, 0.25) } else { c.strip };
    Panel::left("pteron_rail")
        .exact_size(48.0)
        .resizable(false)
        .frame(Frame::NONE.fill(fill))
        .show_inside(ui, |ui| {
            let rect = ui.max_rect();
            let edge = if style == Style::Gx { c.accent.gamma_multiply(0.55) } else { c.line };
            ui.painter().vline(rect.max.x - 0.5, rect.y_range(), Stroke::new(1.0_f32, edge));
            let mut y = rect.min.y + 10.0;
            let mut next = |ui: &mut egui::Ui, icon: Icon, on: bool, key: &str| {
                let spot = Rect::from_center_size(pos2(rect.center().x, y + 18.0), vec2(36.0, 36.0));
                y += 42.0;
                if on {
                    ui.painter().rect_filled(spot, 8.0, c.accent.gamma_multiply(0.22));
                    ui.painter().rect_filled(
                        Rect::from_min_size(pos2(rect.min.x, spot.min.y + 8.0), vec2(3.0, 20.0)),
                        1.5,
                        c.accent,
                    );
                }
                icon_at(ui, spot, icon, c, key)
            };
            if next(ui, Icon::Home, false, "pteron_rail_home").on_hover_text("New tab page").clicked() {
                actions.push(Action::Home);
            }
            let mut panels = vec![
                (RailPanel::Bookmarks, Icon::Star, "Bookmarks"),
                (RailPanel::Downloads, Icon::Download, "Downloads"),
            ];
            if crate::music::now_playing().is_some() {
                panels.push((RailPanel::Music, Icon::Music, "Music"));
            }
            for (panel, icon, tip) in panels {
                let on = *rail == panel;
                if next(ui, icon, on, tip).on_hover_text(tip).clicked() {
                    *rail = if on { RailPanel::Closed } else { panel };
                }
            }
            let cog = Rect::from_center_size(pos2(rect.center().x, rect.max.y - 26.0), vec2(36.0, 36.0));
            if icon_at(ui, cog, Icon::Cog, c, "pteron_rail_settings").on_hover_text("Settings").clicked() {
                actions.push(Action::ToggleSettings);
            }
        });
}

/// The panel next to the side bar: bookmarks, downloads or music.
fn rail_panel(ui: &mut egui::Ui, c: &Colors, rail: &mut RailPanel, model: &Model, actions: &mut Vec<Action>) {
    Panel::left("pteron_rail_panel")
        .exact_size(270.0)
        .resizable(false)
        .frame(Frame::NONE.fill(c.bar).inner_margin(10.0))
        .show_inside(ui, |ui| {
            let rect = ui.max_rect().expand(10.0);
            ui.painter().vline(rect.max.x - 0.5, rect.y_range(), Stroke::new(1.0_f32, c.line));
            ui.horizontal(|ui| {
                let title = match rail {
                    RailPanel::Bookmarks => "Bookmarks",
                    RailPanel::Downloads => "Downloads",
                    _ => "Music",
                };
                ui.label(RichText::new(title).strong().size(15.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new("×").frame(false)).on_hover_text("Close").clicked() {
                        *rail = RailPanel::Closed;
                    }
                });
            });
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match rail {
                RailPanel::Bookmarks if model.bookmarks.is_empty() => {
                    ui.label(RichText::new("No bookmarks yet. Click the star to add one.").weak());
                },
                RailPanel::Bookmarks => sidebar::bookmark_list(ui, model, "", actions),
                RailPanel::Downloads if crate::downloads::snapshot().is_empty() => {
                    ui.label(RichText::new("No downloads yet.").weak());
                },
                RailPanel::Downloads => sidebar::downloads_section(ui),
                _ => sidebar::music_card(ui, 250.0),
            });
        });
}

#[derive(Clone, Copy)]
enum Icon {
    Plus,
    Chevron,
    Dots,
    Lines,
    Home,
    Star,
    Download,
    Music,
    Cog,
}

/// A small round button with a drawn icon (no font glyph needed).
fn icon_button(ui: &mut egui::Ui, icon: Icon, c: &Colors, key: &str) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
    icon_at(ui, rect, icon, c, key)
}

fn icon_at(ui: &mut egui::Ui, rect: Rect, icon: Icon, c: &Colors, key: &str) -> egui::Response {
    let response = ui.interact(rect, Id::new(key), Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect.shrink(1.0), rect.height() / 2.0, c.text.gamma_multiply(0.10));
    }
    let p = rect.center();
    let ink = Stroke::new(1.6_f32, c.text);
    let line = |a: Pos2, b: Pos2| painter.line_segment([a, b], ink);
    match icon {
        Icon::Plus => {
            line(p + vec2(-6.0, 0.0), p + vec2(6.0, 0.0));
            line(p + vec2(0.0, -6.0), p + vec2(0.0, 6.0));
        },
        Icon::Chevron => {
            line(p + vec2(-5.0, -2.0), p + vec2(0.0, 3.0));
            line(p + vec2(0.0, 3.0), p + vec2(5.0, -2.0));
        },
        Icon::Dots => {
            for dy in [-5.0, 0.0, 5.0] {
                painter.circle_filled(p + vec2(0.0, dy), 1.7, c.text);
            }
        },
        Icon::Lines => {
            for dy in [-5.0, 0.0, 5.0] {
                line(p + vec2(-7.0, dy), p + vec2(7.0, dy));
            }
        },
        Icon::Home => {
            let roof = [p + vec2(-8.0, -1.0), p + vec2(0.0, -8.0), p + vec2(8.0, -1.0)];
            line(roof[0], roof[1]);
            line(roof[1], roof[2]);
            painter.rect_stroke(Rect::from_min_max(p + vec2(-5.5, -2.5), p + vec2(5.5, 7.0)), 1.0, ink, StrokeKind::Middle);
        },
        Icon::Star => {
            let points: Vec<Pos2> = (0..10)
                .map(|i| {
                    let angle = i as f32 * std::f32::consts::PI / 5.0 - std::f32::consts::FRAC_PI_2;
                    let radius = if i % 2 == 0 { 8.5 } else { 3.8 };
                    p + vec2(angle.cos(), angle.sin()) * radius
                })
                .collect();
            painter.add(egui::Shape::closed_line(points, ink));
        },
        Icon::Download => {
            line(p + vec2(0.0, -8.0), p + vec2(0.0, 3.0));
            line(p + vec2(-5.0, -2.0), p + vec2(0.0, 3.0));
            line(p + vec2(5.0, -2.0), p + vec2(0.0, 3.0));
            line(p + vec2(-7.0, 7.0), p + vec2(7.0, 7.0));
        },
        Icon::Music => {
            for (i, h) in [7.0, 12.0, 5.0].iter().enumerate() {
                let x = p.x - 6.0 + i as f32 * 5.0;
                painter.rect_filled(Rect::from_min_max(pos2(x, p.y + 6.0 - h), pos2(x + 3.0, p.y + 6.0)), 1.5, c.accent);
            }
        },
        Icon::Cog => {
            painter.circle_stroke(p, 6.5, ink);
            painter.circle_stroke(p, 2.5, ink);
            for i in 0..8 {
                let angle = i as f32 * std::f32::consts::TAU / 8.0;
                let dir = vec2(angle.cos(), angle.sin());
                line(p + dir * 6.5, p + dir * 9.0);
            }
        },
    }
    response
}
