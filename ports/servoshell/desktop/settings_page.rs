/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pteron: a full-window settings page (categories on the left, options on the right),
//! drawn with egui on top of the page area.

use egui::{
    Align2, Area, Color32, Context, FontId, Frame, Id, Key, Order, Rect, RichText, ScrollArea, Sense,
    TextEdit, pos2, vec2,
};

use crate::{pteron, updater};

const NAV_WIDTH: f32 = 220.0;
const CONTENT_WIDTH: f32 = 680.0;
const QUERY_ID: &str = "pteron_settings_query";
const HITS_ID: &str = "pteron_settings_hits";

#[derive(Clone, Copy, Default, PartialEq)]
pub(crate) enum Category {
    #[default]
    Search,
    Appearance,
    Layout,
    Pages,
    Startup,
    Tabs,
    Video,
    Compatibility,
    Privacy,
    About,
}

impl Category {
    const ALL: [Category; 10] = [
        Category::Search,
        Category::Appearance,
        Category::Layout,
        Category::Pages,
        Category::Startup,
        Category::Tabs,
        Category::Video,
        Category::Compatibility,
        Category::Privacy,
        Category::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Category::Search => "Search",
            Category::Appearance => "Appearance",
            Category::Layout => "Layout & buttons",
            Category::Pages => "Web pages",
            Category::Startup => "On startup",
            Category::Tabs => "Tabs & memory",
            Category::Video => "Video",
            Category::Compatibility => "Web compatibility",
            Category::Privacy => "Privacy & data",
            Category::About => "About",
        }
    }
}

/// Things the page asks the app to do (the page itself cannot reach the engine).
pub(crate) enum PageAction {
    /// Reload every open tab (so page styles apply now).
    ReloadTabs,
    ClearCookies,
    ClearCache,
    ForgetSession,
    ClearHistory,
    /// Close Pteron and let the downloaded update replace the files.
    InstallUpdate,
    /// Save a problem report file on the Desktop, with this note from the user.
    SaveReport(String),
    /// Open the page listing known problems in a new tab.
    OpenKnownIssues,
    /// Put the report text on the clipboard.
    CopyReport(String),
    /// Save the bookmarks as an .html file on the Desktop.
    ExportBookmarks,
    /// Import the newest bookmark file found on the Desktop or in Downloads.
    ImportBookmarks,
    /// Open the file chooser for the new tab background picture.
    ChoosePicture,
    /// Open the releases page in a new tab (the manual way to update).
    OpenDownloadPage,
    /// Open the folder chooser for syncing.
    ChooseSyncFolder,
    /// Compare with the sync folder right now.
    SyncNow,
    /// Stop syncing (the file in the folder is left alone).
    StopSync,
}

#[derive(Default)]
pub(crate) struct PageState {
    pub open: bool,
    pub category: Category,
    /// Requests for the app to carry out after this frame.
    pub pending: Vec<PageAction>,
    /// Result of the last request, shown under the buttons.
    pub notice: Option<String>,
    /// Text typed in "Find in Settings".
    pub query: String,
    /// Text typed in "Report a problem".
    pub report_note: String,
}

/// Draw the settings page over `area` when it is open. Returns true if a setting changed.
pub(crate) fn show(
    ctx: &Context,
    page: &mut PageState,
    settings: &mut pteron::Settings,
    area: Rect,
) -> bool {
    if !page.open {
        return false;
    }
    let mut changed = false;
    let mut close = ctx.input(|input| input.key_pressed(Key::Escape));
    let accent = {
        let [r, g, b] = crate::appearance::current_accent();
        Color32::from_rgb(r, g, b)
    };

    Area::new(Id::new("pteron_settings_page"))
        .order(Order::Middle)
        .fixed_pos(area.min)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(ui.visuals().panel_fill)
                .show(ui, |ui| {
                    ui.set_min_size(area.size());
                    ui.set_max_size(area.size());
                    ui.horizontal_top(|ui| {
                        // Left: categories, Firefox-style list.
                        // Note: `allocate_ui` would inherit the horizontal layout of the
                        // parent and put the entries side by side, so ask for top-down.
                        ui.allocate_ui_with_layout(
                            vec2(NAV_WIDTH, area.height()),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                            ui.set_min_height(area.height());
                            ui.add_space(22.0);
                            ui.horizontal(|ui| {
                                ui.add_space(20.0);
                                ui.label(RichText::new("Settings").size(22.0).strong());
                            });
                            ui.add_space(18.0);
                            for category in Category::ALL {
                                let selected = page.query.is_empty() && page.category == category;
                                if nav_row(ui, category.label(), selected, accent).clicked() {
                                    page.category = category;
                                    page.query.clear();
                                }
                            }
                        },
                        );

                        // Right: find box on top, then the options.
                        ui.vertical(|ui| {
                            let width = area.width() - NAV_WIDTH;
                            ui.set_width(width);
                            ui.add_space(18.0);
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.add(
                                    TextEdit::singleline(&mut page.query)
                                        .hint_text("Find in Settings")
                                        .desired_width(300.0)
                                        .margin(vec2(10.0, 6.0)),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.add_space(16.0);
                                        if ui.button("Close  ✕").clicked() {
                                            close = true;
                                        }
                                    },
                                );
                            });
                            ui.add_space(6.0);

                            ctx.data_mut(|data| {
                                data.insert_temp(Id::new(QUERY_ID), page.query.trim().to_lowercase());
                                data.insert_temp(Id::new(HITS_ID), 0usize);
                            });
                            let searching = !page.query.trim().is_empty();

                            // (Each category keeps its own scroll position.)
                            ScrollArea::vertical().id_salt(page.category as u8).auto_shrink([false, false]).show(ui, |ui| {
                                let margin = ((width - CONTENT_WIDTH) / 2.0).max(16.0);
                                ui.horizontal_top(|ui| {
                                    ui.add_space(margin);
                                    ui.vertical(|ui| {
                                        ui.set_width((width - 2.0 * margin - 8.0).min(CONTENT_WIDTH));
                                        ui.add_space(8.0);
                                        let categories: Vec<Category> = if searching {
                                            Category::ALL.to_vec()
                                        } else {
                                            vec![page.category]
                                        };
                                        for category in categories {
                                            if !searching {
                                                ui.label(
                                                    RichText::new(category.label())
                                                        .size(26.0)
                                                        .strong(),
                                                );
                                                ui.add_space(14.0);
                                            }
                                            match category {
                                                Category::Search => {
                                                    changed |= search_section(ui, settings)
                                                },
                                                Category::Appearance => {
                                                    changed |= appearance_section(ui, settings, &mut page.pending, &page.notice)
                                                },
                                                Category::Layout => {
                                                    changed |= layout_section(ui, settings)
                                                },
                                                Category::Pages => {
                                                    changed |= pages_section(ui, settings, &mut page.pending)
                                                },
                                                Category::Startup => {
                                                    changed |= startup_section(ui, settings)
                                                },
                                                Category::Tabs => {
                                                    changed |= tabs_section(ui, settings)
                                                },
                                                Category::Video => {
                                                    changed |= video_section(ui, settings)
                                                },
                                                Category::Compatibility => {
                                                    changed |= compat_section(ui, settings)
                                                },
                                                Category::Privacy => privacy_section(
                                                    ui,
                                                    &mut page.pending,
                                                    &page.notice,
                                                    settings,
                                                    &mut changed,
                                                ),
                                                Category::About => {
                                                    if about_section(ui, &mut page.pending, &mut page.report_note, &page.notice) {
                                                        *settings = pteron::Settings::default();
                                                        changed = true;
                                                    }
                                                },
                                            }
                                        }
                                        if searching &&
                                            ui.data(|data| {
                                                data.get_temp::<usize>(Id::new(HITS_ID))
                                                    .unwrap_or(0)
                                            }) == 0
                                        {
                                            ui.label(
                                                RichText::new("Sorry! No results in Settings for your search.")
                                                    .weak(),
                                            );
                                        }
                                        ui.add_space(30.0);
                                    });
                                });
                            });
                        });
                    });
                });
        });

    if close {
        page.open = false;
        page.query.clear();
    }
    changed
}

/// One entry of the category list: rounded highlight, accent-colored when selected.
fn nav_row(ui: &mut egui::Ui, label: &str, selected: bool, accent: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(NAV_WIDTH - 16.0, 38.0), Sense::click());
    let rect = rect.translate(vec2(8.0, 0.0));
    let visuals = ui.visuals();
    let fill = if selected {
        Some(accent.gamma_multiply(0.18))
    } else if response.hovered() {
        Some(visuals.widgets.hovered.weak_bg_fill)
    } else {
        None
    };
    let text_color = if selected { accent } else { visuals.text_color() };
    let painter = ui.painter();
    if let Some(fill) = fill {
        painter.rect_filled(rect, 8.0, fill);
    }
    painter.text(
        pos2(rect.min.x + 16.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(15.0),
        text_color,
    );
    response
}

/// True when the card should be drawn for the current "Find in Settings" text.
fn matches_query(ui: &egui::Ui, text: &str) -> bool {
    let query = ui
        .data(|data| data.get_temp::<String>(Id::new(QUERY_ID)))
        .unwrap_or_default();
    let show = query.is_empty() || text.to_lowercase().contains(&query);
    if show {
        ui.data_mut(|data| {
            *data.get_temp_mut_or_default::<usize>(Id::new(HITS_ID)) += 1;
        });
    }
    show
}

/// A Firefox-style group: rounded box, bold title, optional description, then the options.
fn card(
    ui: &mut egui::Ui,
    title: &str,
    description: Option<&str>,
    keywords: &str,
    add: impl FnOnce(&mut egui::Ui),
) {
    let haystack = format!("{title} {} {keywords}", description.unwrap_or_default());
    if !matches_query(ui, &haystack) {
        return;
    }
    Frame::group(ui.style())
        .fill(ui.visuals().faint_bg_color)
        .inner_margin(18.0)
        .corner_radius(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).strong().size(16.0));
            if let Some(description) = description {
                ui.add_space(2.0);
                ui.label(RichText::new(description).weak());
            }
            ui.add_space(10.0);
            add(ui);
        });
    ui.add_space(14.0);
}

/// A radio button with a grey explanation indented under it.
fn radio(
    ui: &mut egui::Ui,
    current: &mut String,
    value: &str,
    label: &str,
    description: Option<&str>,
) -> bool {
    let changed = ui
        .radio_value(current, value.to_string(), RichText::new(label).size(14.5))
        .changed();
    if let Some(description) = description {
        ui.horizontal(|ui| {
            ui.add_space(26.0);
            // In a column, so that a long description wraps instead of running off the card.
            ui.vertical(|ui| ui.label(RichText::new(description).small().weak()));
        });
    }
    ui.add_space(4.0);
    changed
}

/// A group of radio buttons: `(value, label, description)` for each.
fn radios(ui: &mut egui::Ui, current: &mut String, options: &[(&str, &str, Option<&str>)]) -> bool {
    options.iter().fold(false, |changed, (value, label, description)| {
        radio(ui, current, value, label, *description) | changed
    })
}

fn search_section(ui: &mut egui::Ui, settings: &mut pteron::Settings) -> bool {
    let mut changed = false;
    card(
        ui,
        "Default search engine",
        Some("Choose the search engine used from the address bar and the new tab page."),
        "search engine duckduckgo google bing brave startpage searxng",
        |ui| {
            for (id, label, _) in pteron::ENGINES {
                changed |= radio(ui, &mut settings.engine, id, label, None);
            }
            changed |= radio(
                ui,
                &mut settings.engine,
                pteron::SEARXNG_ID,
                "SearXNG (my own instance)",
                Some("Use a search server you run yourself."),
            );
        },
    );
    if settings.engine == pteron::SEARXNG_ID {
        card(
            ui,
            "Your SearXNG address",
            Some("Use %s where the search words go. If you leave it out, /search?q=%s is added."),
            "searxng address url instance",
            |ui| {
                changed |= ui
                    .add(
                        TextEdit::singleline(&mut settings.searxng_url)
                            .hint_text("https://search.example.org")
                            .desired_width(f32::INFINITY)
                            .margin(vec2(10.0, 6.0)),
                    )
                    .changed();
            },
        );
    }
    changed |= multi_search_card(ui, settings);
    let target = pteron::search_url_for(settings);
    card(
        ui,
        "Where searches go",
        Some("Searches are sent to this address."),
        "search address url",
        |ui| {
            ui.label(RichText::new(target).small());
        },
    );
    changed
}

fn multi_search_card(ui: &mut egui::Ui, settings: &mut pteron::Settings) -> bool {
    let mut changed = false;
    card(
        ui,
        "Search several engines at once",
        Some("Searches typed in the address bar go to more than one engine."),
        "search several multiple engines at once compare merge tabs",
        |ui| {
            changed |= ui
                .checkbox(&mut settings.multi_search, "Search several engines at once")
                .changed();
            if !settings.multi_search {
                return;
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Warning: this opens more pages or makes more downloads, so it uses more memory \
                     and data. Not recommended on low-memory computers.",
                )
                .small()
                .color(Color32::from_rgb(214, 160, 60)),
            );
            ui.add_space(6.0);
            changed |= radios(ui, &mut settings.multi_mode, &[
                (
                    "tabs",
                    "One tab per engine",
                    Some("Every engine opens in its own tab. Uses the most memory (one extra tab per engine)."),
                ),
                (
                    "page",
                    "One merged results page",
                    Some(
                        "Pteron reads the results of each engine and shows them in one list. Uses less \
                         memory than tabs, but works for DuckDuckGo, Bing, Brave and SearXNG only. If an \
                         engine changes its pages or blocks the request, it is shown as a link instead.",
                    ),
                ),
            ]);
            ui.add_space(6.0);
            ui.label("Engines to use (pick 2 to 4):");
            let mut options: Vec<(String, String)> = pteron::ENGINES
                .iter()
                .map(|(id, label, _)| (id.to_string(), label.to_string()))
                .collect();
            options.push((pteron::SEARXNG_ID.to_string(), "SearXNG (my own instance)".to_string()));
            for (id, label) in options {
                let mut on = settings.multi_engines.contains(&id);
                let full = settings.multi_engines.len() >= crate::multisearch::MAX_ENGINES;
                if ui.add_enabled(on || !full, egui::Checkbox::new(&mut on, label)).changed() {
                    if on {
                        settings.multi_engines.push(id);
                    } else {
                        settings.multi_engines.retain(|e| *e != id);
                    }
                    changed = true;
                }
            }
            if settings.multi_engines.len() < 2 {
                ui.label(RichText::new("Pick at least two engines, or searches use only the default engine.").small().weak());
            }
        },
    );
    changed
}

/// A small preview of a theme: its background, a card, a button and its accent colour.
fn theme_tile(ui: &mut egui::Ui, name: &str, palette: &crate::appearance::Palette, selected: bool) -> egui::Response {
    let c = |rgb: [u8; 3]| Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(104.0, 78.0), egui::Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 10.0, c(palette.bg));
    let ring = if selected {
        c(palette.accent)
    } else if response.hovered() {
        ui.visuals().text_color().gamma_multiply(0.5)
    } else {
        ui.visuals().text_color().gamma_multiply(0.18)
    };
    painter.rect_stroke(rect, 10.0, egui::Stroke::new(if selected { 2.5_f32 } else { 1.0 }, ring), egui::StrokeKind::Inside);
    let card = egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 10.0), egui::vec2(rect.width() - 20.0, 24.0));
    painter.rect_filled(card, 6.0, c(palette.surface));
    painter.circle_filled(card.left_center() + egui::vec2(9.0, 0.0), 4.5, c(palette.accent));
    painter.rect_filled(
        egui::Rect::from_min_size(card.min + egui::vec2(22.0, 9.0), egui::vec2(card.width() - 34.0, 6.0)),
        3.0,
        c(palette.muted).gamma_multiply(0.55),
    );
    painter.text(
        rect.left_bottom() + egui::vec2(10.0, -10.0),
        egui::Align2::LEFT_BOTTOM,
        name,
        egui::FontId::proportional(13.0),
        c(palette.text),
    );
    response
}

/// A round colour sample; `selected` draws a ring around it.
fn swatch(ui: &mut egui::Ui, rgb: [u8; 3], selected: bool, name: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
    let color = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    ui.painter().circle_filled(rect.center(), 11.0, color);
    if selected {
        ui.painter()
            .circle_stroke(rect.center(), 14.0, egui::Stroke::new(2.0_f32, ui.visuals().text_color()));
    } else if response.hovered() {
        ui.painter()
            .circle_stroke(rect.center(), 13.0, egui::Stroke::new(1.5_f32, color.gamma_multiply(0.6)));
    }
    response.on_hover_text(name)
}

fn appearance_section(
    ui: &mut egui::Ui,
    settings: &mut pteron::Settings,
    pending: &mut Vec<PageAction>,
    notice: &Option<String>,
) -> bool {
    let mut changed = false;
    card(
        ui,
        "Theme",
        Some("Colours for the sidebar, settings, buttons and the new tab page."),
        "theme light dark appearance color colour palette midnight forest sunset custom personalize",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                let plain: [(&str, &str, Option<crate::appearance::Palette>); 3] = [
                    ("system", "System", None),
                    ("light", "Light", Some(crate::appearance::LIGHT_PREVIEW)),
                    ("dark", "Dark", Some(crate::appearance::DARK_PREVIEW)),
                ];
                for (id, name, preview) in plain {
                    let preview = preview.unwrap_or_else(|| {
                        if ui.visuals().dark_mode {
                            crate::appearance::DARK_PREVIEW
                        } else {
                            crate::appearance::LIGHT_PREVIEW
                        }
                    });
                    if theme_tile(ui, name, &preview, settings.theme == id).clicked() {
                        settings.theme = id.to_string();
                        changed = true;
                    }
                }
                for (id, name, palette) in crate::appearance::THEMES {
                    if theme_tile(ui, name, &palette, settings.theme == id).clicked() {
                        settings.theme = id.to_string();
                        changed = true;
                    }
                }
                let custom = crate::appearance::parse_hex(&settings.custom_bg).unwrap_or([0x1b, 0x1c, 0x1e]);
                let custom_palette = crate::appearance::palette_for(&pteron::Settings {
                    theme: "custom".to_string(),
                    custom_bg: crate::appearance::to_hex(custom),
                    accent: "auto".to_string(),
                    ..settings.clone()
                })
                .unwrap_or(crate::appearance::DARK_PREVIEW);
                if theme_tile(ui, "Custom", &custom_palette, settings.theme == "custom").clicked() {
                    settings.theme = "custom".to_string();
                    changed = true;
                }
            });
        },
    );
    changed |= theme_editor(ui, settings);
    card(
        ui,
        "Accent colour",
        Some("Highlights, selections, the star and the new tab page glow."),
        "color colour accent palette custom personalize highlight",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                let auto = crate::appearance::palette_for(&pteron::Settings { accent: "auto".to_string(), ..settings.clone() })
                    .map(|p| p.accent)
                    .unwrap_or(crate::appearance::PRESETS[0].2);
                if swatch(ui, auto, settings.accent == "auto", "Theme default").clicked() {
                    settings.accent = "auto".to_string();
                    changed = true;
                }
                for (id, name, rgb) in crate::appearance::PRESETS {
                    if swatch(ui, rgb, settings.accent == id, name).clicked() {
                        settings.accent = id.to_string();
                        changed = true;
                    }
                }
                let custom = crate::appearance::parse_hex(&settings.accent_custom).unwrap_or([0xd6, 0xaa, 0x48]);
                if swatch(ui, custom, settings.accent == "custom", "Your own colour").clicked() {
                    settings.accent = "custom".to_string();
                    changed = true;
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let mut rgb = crate::appearance::parse_hex(&settings.accent_custom).unwrap_or([0xd6, 0xaa, 0x48]);
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    settings.accent_custom = crate::appearance::to_hex(rgb);
                    settings.accent = "custom".to_string();
                    changed = true;
                }
                ui.label(RichText::new("Pick your own colour").weak());
            });
        },
    );
    card(
        ui,
        "New tab background",
        Some("What the page behind the search box looks like."),
        "background wallpaper picture image homepage new tab color gradient",
        |ui| {
            changed |= radios(ui, &mut settings.newtab_bg, &[
                ("theme", "Soft glow in the theme colour", None),
                ("gradient", "Gradient in the theme colour", None),
                ("color", "One colour", None),
            ]);
            if settings.newtab_bg == "color" {
                ui.horizontal(|ui| {
                    ui.add_space(26.0);
                    let mut rgb = crate::appearance::parse_hex(&settings.newtab_color).unwrap_or([0x1b, 0x1c, 0x1e]);
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        settings.newtab_color = crate::appearance::to_hex(rgb);
                        changed = true;
                    }
                });
                ui.add_space(4.0);
            }
            changed |= radio(
                ui,
                &mut settings.newtab_bg,
                "picture",
                "My picture",
                Some("JPG, PNG, WebP or GIF, up to 4 MB."),
            );
            if settings.newtab_bg == "picture" {
                ui.horizontal(|ui| {
                    ui.add_space(26.0);
                    if crate::appearance::picking() {
                        ui.label("Choosing...");
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
                    } else if ui
                        .button(if crate::appearance::has_picture() { "Change picture" } else { "Choose picture" })
                        .clicked()
                    {
                        pending.push(PageAction::ChoosePicture);
                    }
                });
                if !crate::appearance::has_picture() && !crate::appearance::picking() {
                    ui.label(RichText::new("No picture chosen yet, so the soft glow is shown.").small().weak());
                }
            }
            if let Some(notice) = notice {
                ui.add_space(6.0);
                ui.label(notice);
            }
        },
    );
    changed
}

fn startup_section(ui: &mut egui::Ui, settings: &mut pteron::Settings) -> bool {
    let mut changed = false;
    card(
        ui,
        "When Pteron starts",
        Some("Pinned tabs always come back."),
        "startup start launch homepage restore session new tab",
        |ui| {
            changed |= radios(ui, &mut settings.startup, &[
                ("newtab", "Open a new tab", None),
                (
                    "restore",
                    "Continue where I left off",
                    Some(
                        "Tabs come back asleep to save memory: the tab you were on loads right away, \
                         the others load when you click them.",
                    ),
                ),
                ("custom", "Open a page I choose", None),
            ]);
            if settings.startup == "custom" {
                ui.add_space(4.0);
                changed |= ui
                    .add(
                        TextEdit::singleline(&mut settings.homepage_url)
                            .hint_text("https://example.org")
                            .desired_width(f32::INFINITY)
                            .margin(vec2(10.0, 6.0)),
                    )
                    .changed();
            }
        },
    );
    card(
        ui,
        "Tiles on the new tab page",
        Some("The row of site shortcuts under the search box."),
        "new tab tiles shortcuts bookmarks most visited sites frequent",
        |ui| {
            changed |= radios(ui, &mut settings.newtab_tiles, &[
                ("off", "Show nothing", None),
                ("mine", "Only my shortcuts", Some("The ones you add below or with + on the new tab page.")),
                ("bookmarks", "My shortcuts and bookmarks", Some("Star a page to put it here.")),
                ("frequent", "My shortcuts, bookmarks and the sites I visit most", None),
            ]);
            changed |= shortcuts_editor(ui, settings);
        },
    );
    changed
}

fn tabs_section(ui: &mut egui::Ui, settings: &mut pteron::Settings) -> bool {
    let mut changed = false;
    card(
        ui,
        "Sleeping tabs",
        Some(
            "A sleeping tab frees its memory and shows as \"zZ\" in the sidebar. Click it to wake \
             it; the page reloads. Tabs that are playing audio or video never sleep.",
        ),
        "sleep memory ram tabs idle",
        |ui| {
            let mut enabled = settings.tab_sleep_minutes > 0;
            if ui
                .checkbox(&mut enabled, "Put unused background tabs to sleep")
                .changed()
            {
                settings.tab_sleep_minutes = if enabled { 5 } else { 0 };
                changed = true;
            }
            if enabled {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(26.0);
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut settings.tab_sleep_minutes)
                                .range(1..=240)
                                .prefix("Sleep after ")
                                .suffix(" min unused"),
                        )
                        .changed();
                });
            }
        },
    );
    changed |= memory_card(ui, settings);
    changed |= compat_tab_card(ui, settings);
    changed
}

/// A list of site names with Remove buttons and a box to add one. Returns true when it changed.
fn site_list(ui: &mut egui::Ui, id: &str, title: &str, sites: &mut Vec<String>) -> bool {
    let mut changed = false;
    ui.label(title);
    let mut remove = None;
    for (index, site) in sites.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(site);
            if ui.small_button("Remove").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        sites.remove(index);
        changed = true;
    }
    let input_id = egui::Id::new(id);
    let mut text = ui.data_mut(|d| d.get_temp::<String>(input_id)).unwrap_or_default();
    ui.horizontal(|ui| {
        let reply = ui.add(egui::TextEdit::singleline(&mut text).hint_text("example.com").desired_width(180.0));
        let entered = reply.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if (ui.button("Add").clicked() || entered) && !text.trim().is_empty() {
            sites.push(std::mem::take(&mut text));
            *sites = crate::compat_view::clean_sites(std::mem::take(sites));
            changed = true;
            if entered {
                reply.request_focus();
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(input_id, text));
    changed
}

fn compat_tab_card(ui: &mut egui::Ui, settings: &mut pteron::Settings) -> bool {
    let mut changed = false;
    card(
        ui,
        "Compatibility tab",
        Some(
            "Some sites (Cloudflare's browser check, for example) cannot be passed by Pteron's \
             engine. A compatibility tab shows such a page with your system's own web engine \
             (WebView2 on Windows) inside a normal tab. It uses extra memory only while it is \
             open, and it keeps its own cookies, so you sign in separately there. Pteron's ad \
             blocker and passwords do not apply inside it.",
        ),
        "compatibility tab cloudflare webview2 browser check just a moment",
        |ui| {
            changed |= ui
                .checkbox(&mut settings.compat_tab_handoff, "Hand over to the light engine after the check (experimental)")
                .changed();
            ui.label(
                RichText::new(
                    "When a site's browser check passes, Pteron copies the pass (a cookie) to its own engine and \
                     closes the heavy one, so it only runs for a few seconds. If the site does not accept it, \
                     the page goes back to a compatibility tab. While this is on, Pteron presents itself with \
                     the system engine's browser name, and the other compatibility options below are switched off.",
                )
                .small()
                .weak(),
            );
            ui.add_space(6.0);
            changed |= site_list(
                ui,
                "compat_keep_input",
                "Sites that never use Pteron's own engine (it draws them badly). They always open in a compatibility tab, in every mode:",
                &mut settings.compat_tab_keep_sites,
            );
            ui.add_space(6.0);
            ui.add_enabled_ui(!settings.compat_tab_handoff, |ui| {
                changed |= radios(ui, &mut settings.compat_tab_mode, &[
                    ("auto", "Switch automatically", Some("A browser-check page moves to a compatibility tab by itself.")),
                    ("ask", "Ask me", Some("Pteron asks first.")),
                    ("off", "Off", Some("Only the notice with Open in default browser.")),
                ]);
                ui.add_space(6.0);
                changed |= site_list(ui, "compat_site_input", "Sites that always open in a compatibility tab:", &mut settings.compat_tab_sites);
            });
            ui.add_space(6.0);
            changed |= ui
                .checkbox(&mut settings.compat_tab_no_gpu, "Use less memory (experimental)")
                .changed();
            ui.label(
                RichText::new(
                    "Turns off the engine's graphics helper. Saves memory (often around 50 MB), but scrolling \
                     and video can be slower. Applies the next time Pteron starts.",
                )
                .small()
                .weak(),
            );
        },
    );
    changed
}

fn memory_card(ui: &mut egui::Ui, settings: &mut pteron::Settings) -> bool {
    let mut changed = false;
    card(
        ui,
        "Memory saver",
        Some(
            "For older or low-memory computers. Pteron uses less memory, but some things get \
             slower: videos buffer less and may pause on a slow connection, background tabs \
             sleep sooner and reload when you come back, and pages may load a little slower. \
             Changing the level needs a restart of Pteron. At every level, background tabs go to \
             sleep early while the computer is short of memory.",
        ),
        "memory saver low ram reduce usage old slow computer",
        |ui| {
            changed |= radios(ui, &mut settings.memory_mode, &[
                ("normal", "Normal", Some("Best speed. Each tab keeps its last 2 pages ready for Back.")),
                (
                    "low",
                    "Low memory",
                    Some(
                        "Smaller video buffer, background tabs sleep after 3 minutes, memory is handed \
                         back to the system when Pteron is in the background, fewer helper threads, \
                         only the last page kept ready for Back, at most 8 background tabs awake.",
                    ),
                ),
                (
                    "lowest",
                    "Lowest memory",
                    Some(
                        "Very small video buffer, tabs sleep after 1 minute, one page-layout thread, \
                         small cache, only the last page kept ready for Back, at most 4 background \
                         tabs awake, animations at 30 frames a second. Pages and videos start \
                         noticeably slower.",
                    ),
                ),
            ]);
            if let Some(mb) = crate::memsaver::process_mb() {
                ui.add_space(6.0);
                // The system web engine behind compatibility tabs runs as separate programs.
                let more = match crate::memsaver::helpers_mb() {
                    0 => String::new(),
                    helpers => format!(
                        " Compatibility tabs (the system web engine) use about {helpers} MB more: {} MB in all.",
                        mb + helpers
                    ),
                };
                ui.label(format!("Pteron is using about {mb} MB right now.{more}"));
            }
        },
    );
    changed
}

fn video_section(ui: &mut egui::Ui, settings: &mut pteron::Settings) -> bool {
    let mut changed = false;
    card(
        ui,
        "YouTube",
        Some(
            "YouTube's own website needs web features Pteron does not have yet. With this on, \
             YouTube search, channels and videos open in simple Pteron pages instead. Videos \
             play up to 1080p (they start at 720p to save memory). Nothing needs to be installed.",
        ),
        "youtube video player watch quality",
        |ui| {
            changed |= ui
                .checkbox(&mut settings.youtube_player, "Browse and play YouTube in Pteron's own pages")
                .changed();
            changed |= ui
                .checkbox(&mut settings.fast_start, "Start long videos sooner (first two minutes at once, then the whole video)")
                .changed();
        },
    );
    card(
        ui,
        "Video decoding",
        Some(
            "Lets your graphics card decode video instead of the processor. Smoother playback, \
             less CPU use and longer battery life. Turn it off if videos show green or broken \
             pictures. Takes effect after you restart Pteron.",
        ),
        "hardware acceleration gpu video decode graphics card battery",
        |ui| {
            changed |= ui
                .checkbox(&mut settings.hw_video_decode, "Use hardware video decoding")
                .changed();
        },
    );
    changed
}

fn compat_section(ui: &mut egui::Ui, settings: &mut pteron::Settings) -> bool {
    let mut changed = false;
    card(
        ui,
        "Newer web features",
        Some(
            "Servo, the engine inside Pteron, has these features built in but switched off \
             because they are still being finished. Turning one on can make a site work that \
             did not, but it can also make a page slower or crash. Pages reload when you change \
             a switch. If Pteron does not close properly, these stay off for the next session.",
        ),
        "compatibility experimental features indexeddb intersection observer fontface canvas storage container queries web apps",
        |ui| {
            if crate::compat::safe_mode() && !settings.compat.is_empty() {
                ui.label(
                    RichText::new("Pteron did not close properly last time, so these are off for this session. Restart Pteron to use them again.")
                        .color(ui.visuals().warn_fg_color),
                );
                ui.add_space(6.0);
            }
            for feature in crate::compat::FEATURES {
                let mut on = settings.compat.iter().any(|id| id == feature.id);
                let response = ui.checkbox(&mut on, feature.title).on_hover_text(feature.note);
                ui.label(RichText::new(feature.note).small().weak());
                ui.add_space(4.0);
                if response.changed() {
                    settings.compat.retain(|id| id != feature.id);
                    if on {
                        settings.compat.push(feature.id.to_string());
                    }
                    changed = true;
                }
            }
        },
    );
    changed
}

fn privacy_section(
    ui: &mut egui::Ui,
    pending: &mut Vec<PageAction>,
    notice: &Option<String>,
    settings: &mut pteron::Settings,
    changed: &mut bool,
) {
    card(
        ui,
        "History",
        Some(
            "Remembered pages power the suggestions under the address bar and the shortcuts \
             on the new tab page. Everything stays on this computer.",
        ),
        "history privacy suggestions remember visited pages",
        |ui| {
            *changed |= ui
                .checkbox(&mut settings.history_enabled, "Remember the pages I visit")
                .changed();
            ui.add_space(6.0);
            if ui.button("Clear browsing history").clicked() {
                pending.push(PageAction::ClearHistory);
            }
        },
    );
    card(
        ui,
        "Passwords",
        Some(
            "Pteron asks before saving a login, and fills it only when you click. Saved passwords \
             are locked with your Windows account, kept only on this computer, and never synced.",
        ),
        "passwords logins saved autofill sign in remember",
        |ui| {
            if !cfg!(windows) {
                ui.label("Saved passwords work on Windows only for now. On this system Pteron does not offer to save them.");
                return;
            }
            let mut on = crate::passwords::enabled();
            if ui.checkbox(&mut on, "Offer to save passwords").changed() {
                crate::passwords::set_enabled(on);
            }
            let logins = crate::passwords::list();
            ui.add_space(6.0);
            if logins.is_empty() {
                ui.label("No saved logins yet.");
            }
            let mut remove = None;
            for (index, (site, user)) in logins.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(if user.is_empty() { site.clone() } else { format!("{site} - {user}") });
                    if ui.small_button("Delete").clicked() {
                        remove = Some(index);
                    }
                });
            }
            if let Some(index) = remove {
                crate::passwords::delete(index);
            }
            if logins.len() > 1 && ui.button("Delete all saved logins").clicked() {
                crate::passwords::delete_all();
            }
            let never = crate::passwords::never_list();
            if !never.is_empty() {
                ui.add_space(8.0);
                ui.label("Never ask on:");
                let mut allow = None;
                for (index, site) in never.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(site);
                        if ui.small_button("Ask again").clicked() {
                            allow = Some(index);
                        }
                    });
                }
                if let Some(index) = allow {
                    crate::passwords::allow_again(index);
                }
            }
        },
    );
    card(
        ui,
        "Sync between computers",
        Some(
            "Keeps your bookmarks and settings the same on two computers through a folder that \
             both have, such as one inside OneDrive, Google Drive or Dropbox. Pteron only writes \
             one small file there. History, passwords and cookies are never synced.",
        ),
        "sync synchronize devices computers phone laptop bookmarks settings onedrive drive dropbox backup",
        |ui| {
            if settings.sync_folder.trim().is_empty() {
                if crate::sync::choosing() {
                    ui.label("Choosing...");
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
                } else if ui.button("Choose sync folder").clicked() {
                    pending.push(PageAction::ChooseSyncFolder);
                }
            } else {
                ui.label(RichText::new(format!("Folder: {}", settings.sync_folder)).weak());
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Sync now").clicked() {
                        pending.push(PageAction::SyncNow);
                    }
                    if ui.button("Change folder").clicked() {
                        pending.push(PageAction::ChooseSyncFolder);
                    }
                    if ui.button("Stop syncing").clicked() {
                        pending.push(PageAction::StopSync);
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Changes are saved to the folder as you make them. Another computer's changes are picked up when Pteron starts, or when you press Sync now.")
                        .small()
                        .weak(),
                );
            }
        },
    );
    card(
        ui,
        "Secure connections",
        Some("Sites that use plain http can be read and changed by others on the same network."),
        "https http secure security warning connection first upgrade",
        |ui| {
            *changed |= ui
                .checkbox(&mut settings.https_first, "Use the secure version (https) of a site when it has one")
                .changed();
            *changed |= ui
                .checkbox(&mut settings.https_warning, "Warn me before opening http sites")
                .changed();
        },
    );
    card(
        ui,
        "Tracking",
        Some(
            "Links often carry extra parts (utm_source, fbclid and similar) that only tell the \
             site where you came from. Websites can also be told you do not want to be tracked; \
             some honour it, many ignore it.",
        ),
        "tracking track do not track dnt gpc global privacy control utm fbclid links",
        |ui| {
            *changed |= ui
                .checkbox(&mut settings.strip_tracking, "Remove tracking parts from addresses")
                .changed();
            *changed |= ui
                .checkbox(&mut settings.privacy_signals, "Send Do Not Track and Global Privacy Control (after a restart)")
                .changed();
        },
    );
    card(
        ui,
        "Clear on exit",
        Some("Forget what you looked at each time you close Pteron."),
        "clear exit close history cache privacy forget",
        |ui| {
            *changed |= ui
                .checkbox(&mut settings.clear_on_exit, "Clear history and cache when I close Pteron")
                .changed();
            ui.add_space(2.0);
            ui.label(RichText::new("Bookmarks, settings and sign-ins (cookies) are kept.").small().weak());
        },
    );
    card(
        ui,
        "Ads and trackers",
        Some("Cancels requests to well-known advertising and tracking networks. Pages load faster and use less memory."),
        "ads adblock ad blocker tracker tracking privacy block",
        |ui| {
            *changed |= ui
                .checkbox(&mut settings.adblock, "Block ads and trackers")
                .changed();
            ui.add_enabled_ui(settings.adblock, |ui| {
                *changed |= ui
                    .checkbox(&mut settings.adblock_filters, "Use the full EasyList and EasyPrivacy lists")
                    .changed();
            });
            ui.label(
                RichText::new(
                    "Blocks far more ads and trackers, but uses roughly 15 to 30 MB more memory. \
                     The lists are downloaded from easylist.to and refreshed weekly.",
                )
                .small()
                .weak(),
            );
            if settings.adblock && settings.adblock_filters {
                ui.label(RichText::new(crate::filters::status()).small().weak());
            }
            let count = crate::adblock::blocked_count();
            if count > 0 {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("Blocked {count} requests since Pteron started.")).weak());
            }
            ui.add_space(2.0);
            ui.label(RichText::new("If a page looks broken, turn this off and reload.").small().weak());
        },
    );
    card(
        ui,
        "Bookmarks",
        Some(
            "Move your bookmarks between browsers or computers. Export saves Pteron-bookmarks.html \
             on the Desktop. To import, export a bookmark file from Chrome, Edge or Firefox into \
             Downloads or the Desktop, then press Import.",
        ),
        "bookmarks import export chrome firefox edge transfer move",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Export bookmarks").clicked() {
                    pending.push(PageAction::ExportBookmarks);
                }
                if ui.button("Import bookmarks").clicked() {
                    pending.push(PageAction::ImportBookmarks);
                }
            });
        },
    );
    card(
        ui,
        "Cookies, cache and saved tabs",
        None,
        "privacy clear cookies cache data forget session",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Clear cookies and site data").clicked() {
                    pending.push(PageAction::ClearCookies);
                }
                if ui.button("Clear cache").clicked() {
                    pending.push(PageAction::ClearCache);
                }
                if ui.button("Forget saved tabs").clicked() {
                    pending.push(PageAction::ForgetSession);
                }
            });
            if let Some(notice) = notice {
                ui.add_space(8.0);
                ui.label(notice);
            }
        },
    );
}

/// Returns true when the user asked to reset all settings.
/// "Updates": look for a newer release on GitHub, download it, restart into it.
fn updates_card(ui: &mut egui::Ui, pending: &mut Vec<PageAction>) {
    static LAST_ERROR: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    if !updater::supported() {
        card(
            ui,
            "Updates",
            Some("Pteron updates itself on Windows. On this system, update it the way you installed it."),
            "update updates upgrade version check new",
            |_| {},
        );
        return;
    }
    updater::check_once();
    let status = updater::status();
    if updater::busy() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(400));
    }
    card(
        ui,
        "Updates",
        Some("Pteron looks for new versions on GitHub when you open this page. Nothing is installed without your say-so."),
        "update updates upgrade version check new download install",
        |ui| {
            if let Some(error) = LAST_ERROR.get_or_init(updater::take_last_error) {
                ui.label(RichText::new(format!("The last update did not finish: {error}")).color(Color32::from_rgb(214, 120, 90)));
                ui.add_space(6.0);
            }
            if let Some(error) = updater::install_error() {
                ui.label(RichText::new(error).color(Color32::from_rgb(214, 120, 90)));
                ui.add_space(6.0);
            }
            match status {
                updater::Status::Idle | updater::Status::Checking => {
                    ui.label("Looking for updates...");
                },
                updater::Status::UpToDate => {
                    ui.label(format!("You have the newest version (build {}).", updater::BUILD));
                    ui.add_space(8.0);
                    if ui.button("Check again").clicked() {
                        updater::check_async();
                    }
                },
                updater::Status::Available(release) => {
                    ui.label(RichText::new(format!("{} is available (you have build {}).", release.name, updater::BUILD)).strong());
                    if !release.notes.trim().is_empty() {
                        ui.add_space(4.0);
                        ui.label(RichText::new(release.notes.trim()).weak());
                    }
                    ui.add_space(8.0);
                    if ui.button("Download update").clicked() {
                        updater::download_async(release);
                    }
                },
                updater::Status::Downloading { done, total } => {
                    let fraction = if total > 0 { (done as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
                    ui.label(format!("Downloading... {} of {} MB", done / 1_000_000, total / 1_000_000));
                    ui.add_space(6.0);
                    ui.add(egui::ProgressBar::new(fraction).desired_width(320.0));
                },
                updater::Status::Ready(_) => {
                    ui.label(RichText::new("The update is downloaded and checked.").strong());
                    ui.label("Pteron will close, install it and open again. Your tabs come back.");
                    ui.add_space(8.0);
                    if ui.button("Restart and update").clicked() {
                        pending.push(PageAction::InstallUpdate);
                    }
                },
                updater::Status::Failed(message) => {
                    ui.label(RichText::new(message).color(Color32::from_rgb(214, 120, 90)));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Try again").clicked() {
                            updater::check_async();
                        }
                        if ui
                            .button("Open the download page")
                            .on_hover_text("Download the installer yourself and run it")
                            .clicked()
                        {
                            pending.push(PageAction::OpenDownloadPage);
                        }
                    });
                },
            }
        },
    );
}

fn about_section(
    ui: &mut egui::Ui,
    pending: &mut Vec<PageAction>,
    report_note: &mut String,
    notice: &Option<String>,
) -> bool {
    let mut reset = false;
    card(
        ui,
        pteron::APP_NAME,
        Some("Rendering engine: modified Servo (MPL-2.0). Source: github.com/Sm0r3s-source/pteron"),
        "about version servo",
        |ui| {
            ui.label(format!("Version 0.{}", updater::BUILD));
        },
    );
    updates_card(ui, pending);
    card(
        ui,
        "Report a problem",
        Some("Saves a small file on your Desktop (versions, your computer, your settings and the newest log). Pteron sends nothing by itself: you send the file to whoever gave you Pteron."),
        "report problem bug feedback issue help known",
        |ui| {
            ui.label("What went wrong? (optional)");
            ui.add(
                egui::TextEdit::multiline(report_note)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Save report to Desktop").clicked() {
                    pending.push(PageAction::SaveReport(report_note.clone()));
                }
                if ui.button("Copy details").clicked() {
                    pending.push(PageAction::CopyReport(report_note.clone()));
                }
                if ui.button("Known problems").clicked() {
                    pending.push(PageAction::OpenKnownIssues);
                }
            });
            if let Some(text) = notice &&
                (text.starts_with("Saved:") || text.starts_with("Could not save") || text.starts_with("Copied"))
            {
                ui.add_space(6.0);
                ui.label(RichText::new(text).small());
            }
        },
    );
    card(
        ui,
        "Settings file",
        Some("Your settings are stored here."),
        "reset settings file default",
        |ui| {
            ui.label(RichText::new(pteron::settings_file_path()).small());
            ui.add_space(8.0);
            reset = ui.button("Reset all settings to default").clicked();
        },
    );
    reset
}

// ---------------------------------------------------------------- Pteron: customization

/// Edit every colour of the custom theme, starting from any theme.
fn theme_editor(ui: &mut egui::Ui, settings: &mut pteron::Settings) -> bool {
    let mut changed = false;
    card(
        ui,
        "Theme editor",
        Some("Make your own theme: start from any theme, then change each colour."),
        "theme editor custom colour color background text cards make own palette",
        |ui| {
            ui.horizontal(|ui| {
                ui.label("Start from");
                let mut start: Option<crate::appearance::Palette> = None;
                egui::ComboBox::from_id_salt("pteron_theme_start")
                    .selected_text("Choose a theme")
                    .show_ui(ui, |ui| {
                        for (name, palette) in [("Light", crate::appearance::LIGHT_PREVIEW), ("Dark", crate::appearance::DARK_PREVIEW)]
                            .into_iter()
                            .chain(crate::appearance::THEMES.iter().map(|(_, name, p)| (*name, *p)))
                        {
                            if ui.selectable_label(false, name).clicked() {
                                start = Some(palette);
                            }
                        }
                    });
                if let Some(palette) = start {
                    crate::appearance::start_custom_from(settings, &palette);
                    changed = true;
                }
            });
            if settings.theme != "custom" {
                ui.label(RichText::new("Choose a theme to start from (or the Custom tile above) to edit its colours.").small().weak());
                return;
            }
            ui.add_space(6.0);
            // The colours worked out from the background alone, shown for "Automatic" ones.
            let auto = crate::appearance::palette_for(&pteron::Settings {
                theme: "custom".into(),
                custom_bg: settings.custom_bg.clone(),
                ..Default::default()
            });
            let look = &mut settings.look;
            let rows: [(&str, &mut String, bool); 4] = [
                ("Background", &mut settings.custom_bg, false),
                ("Cards, fields and the toolbar", &mut look.custom_surface, true),
                ("Text", &mut look.custom_text, true),
                ("Soft text", &mut look.custom_muted, true),
            ];
            for (index, (label, value, can_auto)) in rows.into_iter().enumerate() {
                ui.horizontal(|ui| {
                    let fallback = auto.map(|p| [p.bg, p.surface, p.text, p.muted][index]).unwrap_or([0x80, 0x80, 0x80]);
                    let mut rgb = crate::appearance::parse_hex(value).unwrap_or(fallback);
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        *value = crate::appearance::to_hex(rgb);
                        changed = true;
                    }
                    ui.label(label);
                    if can_auto && !value.is_empty() && ui.small_button("Automatic").on_hover_text("Work it out from the background").clicked() {
                        value.clear();
                        changed = true;
                    }
                });
            }
            ui.horizontal(|ui| {
                let mut rgb = crate::appearance::accent_rgb(settings);
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    settings.accent_custom = crate::appearance::to_hex(rgb);
                    settings.accent = "custom".into();
                    changed = true;
                }
                ui.label("Accent");
            });
        },
    );
    changed
}

/// A small drawing of a browser layout, for picking one.
fn layout_tile(ui: &mut egui::Ui, id: &str, name: &str, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(150.0, 112.0), Sense::click());
    let v = ui.visuals();
    let text = v.text_color();
    let accent = {
        let [r, g, b] = crate::appearance::current_accent();
        Color32::from_rgb(r, g, b)
    };
    let gx = id == "gx";
    let (back, chrome, page) = if gx {
        (Color32::from_rgb(14, 11, 19), Color32::from_rgb(27, 21, 33), Color32::from_rgb(40, 34, 48))
    } else {
        (text.gamma_multiply(0.16), text.gamma_multiply(0.07), v.extreme_bg_color)
    };
    let ink = if gx { Color32::from_rgb(250, 30, 78) } else { text.gamma_multiply(0.45) };
    let p = ui.painter();
    let window = Rect::from_min_size(rect.min, vec2(150.0, 88.0));
    p.rect_filled(window, 8.0, page);
    let ring = if selected { accent } else if response.hovered() { text.gamma_multiply(0.5) } else { text.gamma_multiply(0.2) };
    let box_at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(window.min + vec2(x, y), vec2(w, h));
    match id {
        "sidebar" => {
            p.rect_filled(box_at(0.0, 0.0, 44.0, 88.0), egui::CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, back);
            p.rect_filled(box_at(6.0, 8.0, 32.0, 6.0), 3.0, page);
            for (i, y) in [24.0, 34.0, 44.0, 54.0].iter().enumerate() {
                p.rect_filled(box_at(6.0, *y, 32.0, 6.0), 3.0, if i == 0 { accent.gamma_multiply(0.7) } else { ink.gamma_multiply(0.6) });
            }
        },
        _ => {
            let rail = if matches!(id, "opera" | "gx") { 14.0 } else { 0.0 };
            let (top_x, rail_y) = if gx { (rail, 0.0) } else { (0.0, 32.0) };
            p.rect_filled(box_at(top_x, 0.0, 150.0 - top_x, 18.0), egui::CornerRadius { nw: if gx { 0 } else { 8 }, ne: 8, sw: 0, se: 0 }, back);
            p.rect_filled(box_at(top_x, 18.0, 150.0 - top_x, 14.0), 0.0, chrome);
            for i in 0..3 {
                let x = top_x + 6.0 + i as f32 * 30.0;
                let tab = if id == "firefox" { box_at(x, 4.0, 27.0, 11.0) } else { box_at(x, 5.0, 28.0, 13.0) };
                let corners = if id == "firefox" { egui::CornerRadius::same(4) } else { egui::CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 } };
                if i == 0 {
                    p.rect_filled(tab, corners, chrome);
                    if gx {
                        p.rect_filled(box_at(x, 16.0, 28.0, 2.0), 0.0, ink);
                    }
                } else {
                    p.rect_filled(box_at(x + 5.0, 9.0, 16.0, 3.0), 1.5, ink.gamma_multiply(0.6));
                }
            }
            let (field_x, field_w) = if id == "firefox" { (top_x + 36.0, 78.0 - top_x / 2.0) } else { (top_x + 24.0, 112.0 - top_x) };
            let radius = if id == "chrome" { 4 } else { 2 };
            p.rect_filled(box_at(field_x, 21.0, field_w, 8.0), radius, if gx { page } else { text.gamma_multiply(0.12) });
            if rail > 0.0 {
                p.rect_filled(box_at(0.0, rail_y, rail, 88.0 - rail_y), egui::CornerRadius { nw: if gx { 8 } else { 0 }, sw: 8, ne: 0, se: 0 }, back);
                for i in 0..3 {
                    p.circle_filled(window.min + vec2(rail / 2.0, rail_y + 10.0 + i as f32 * 12.0), 2.5, ink);
                }
            }
        },
    }
    p.rect_stroke(window, 8.0, egui::Stroke::new(if selected { 2.5_f32 } else { 1.0 }, ring), egui::StrokeKind::Inside);
    p.text(
        rect.left_bottom() + vec2(4.0, -4.0),
        Align2::LEFT_BOTTOM,
        name,
        FontId::proportional(13.5),
        if selected { accent } else { text },
    );
    response
}

fn layout_section(ui: &mut egui::Ui, settings: &mut pteron::Settings) -> bool {
    let mut changed = false;
    card(
        ui,
        "Browser layout",
        Some(
            "Make Pteron look like the browser you are used to. Choosing a layout also picks matching \
             colours and corners; you can change them afterwards.",
        ),
        "layout style chrome firefox opera gx gaming tabs top sidebar browser look design",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
                for (id, name, _, theme, roundness) in crate::look::LAYOUTS {
                    if layout_tile(ui, id, name, settings.look.layout == id).clicked() && settings.look.layout != id {
                        settings.look.layout = id.to_string();
                        settings.look.roundness = roundness;
                        if !theme.is_empty() {
                            settings.theme = theme.to_string();
                            settings.accent = "auto".to_string();
                        }
                        changed = true;
                    }
                }
            });
            if let Some((.., description, _, _)) = crate::look::LAYOUTS.iter().find(|(id, ..)| *id == settings.look.layout) {
                ui.add_space(6.0);
                ui.label(RichText::new(*description).weak());
            }
        },
    );
    let look = &mut settings.look;
    if !look.top_tabs() {
        card(
            ui,
            "Sidebar",
            Some("When it is not always shown, the sidebar slides in when the mouse touches the window edge."),
            "sidebar tabs always show pin right left side width",
            |ui| {
                changed |= ui.checkbox(&mut settings.sidebar_pinned, "Always show the sidebar").changed();
                changed |= ui.checkbox(&mut look.sidebar_right, "Put the sidebar on the right").changed();
                changed |= ui.add(egui::Slider::new(&mut look.sidebar_width, 200..=420).text("Width").suffix(" px")).changed();
            },
        );
    }
    card(
        ui,
        "Tabs",
        None,
        "tabs icons favicon close button bookmarks bar",
        |ui| {
            changed |= ui.checkbox(&mut look.tab_icons, "Show site icons on tabs").changed();
            changed |= ui.checkbox(&mut look.close_always, "Show a close button on every tab (not only under the mouse)").changed();
            if look.top_tabs() {
                changed |= ui.checkbox(&mut look.bookmarks_bar, "Show the bookmarks bar under the toolbar").changed();
            }
        },
    );
    card(
        ui,
        "Buttons",
        Some("Which buttons are shown next to the address bar."),
        "toolbar buttons hide show back forward reload home star reader copy new window experimental",
        |ui| {
            for (id, name) in crate::look::BUTTONS {
                let mut shown = look.shows(id);
                if ui.checkbox(&mut shown, name).changed() {
                    look.hidden_buttons.retain(|hidden| hidden != id);
                    if !shown {
                        look.hidden_buttons.push(id.to_string());
                    }
                    changed = true;
                }
            }
        },
    );
    card(
        ui,
        "Size and shape",
        Some("How big the interface is, how much room there is between things, and how round the corners are."),
        "size scale zoom interface bigger smaller density compact roomy spacing corners round square",
        |ui| {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(egui::Slider::new(&mut look.scale, 75..=175).step_by(5.0).text("Interface size").suffix("%"))
                    .changed();
                if look.scale != 100 && ui.small_button("100%").clicked() {
                    look.scale = 100;
                    changed = true;
                }
            });
            ui.add_space(6.0);
            changed |= radios(ui, &mut look.density, &[
                ("compact", "Compact", Some("More tabs and bookmarks fit.")),
                ("normal", "Normal", None),
                ("roomy", "Roomy", Some("Bigger targets, easier on touch screens.")),
            ]);
            changed |= ui.add(egui::Slider::new(&mut look.roundness, 0..=16).text("Corner roundness")).changed();
        },
    );
    changed
}

fn pages_section(ui: &mut egui::Ui, settings: &mut pteron::Settings, pending: &mut Vec<PageAction>) -> bool {
    let mut changed = false;
    let look = &mut settings.look;
    card(
        ui,
        "Dark mode for web pages",
        Some(
            "Pages that have their own dark look use it. Other light pages are turned dark, while \
             pictures and videos keep their colours. Turned-dark pages take a little more power to scroll.",
        ),
        "dark mode night web pages sites invert black",
        |ui| {
            changed |= ui.checkbox(&mut look.dark_pages, "Make web pages dark").changed();
            if look.dark_pages {
                ui.add_space(6.0);
                changed |= site_list(ui, "pteron_dark_skip", "Sites left as they are:", &mut look.dark_skip);
                ui.label(RichText::new("The half-moon button next to the address bar switches this for the site you are on.").small().weak());
            }
        },
    );
    card(
        ui,
        "Your styles for sites",
        Some("Your own CSS for a site and its subdomains, or * for every site. Applies when a page loads."),
        "css style user styles custom site stylesheet",
        |ui| {
            let mut remove = None;
            for (index, style) in look.site_styles.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    changed |= ui
                        .add(TextEdit::singleline(&mut style.site).hint_text("example.com or *").desired_width(220.0))
                        .changed();
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                });
                changed |= ui
                    .add(
                        TextEdit::multiline(&mut style.css)
                            .code_editor()
                            .hint_text("body { font-size: 18px; }")
                            .desired_rows(4)
                            .desired_width(f32::INFINITY),
                    )
                    .changed();
                ui.add_space(8.0);
            }
            if let Some(index) = remove {
                look.site_styles.remove(index);
                changed = true;
            }
            ui.horizontal(|ui| {
                if ui.button("Add a site style").clicked() {
                    look.site_styles.push(crate::look::SiteStyle::default());
                    changed = true;
                }
                if !look.site_styles.is_empty() && ui.button("Apply to open tabs").on_hover_text("Reloads every open tab").clicked() {
                    pending.push(PageAction::ReloadTabs);
                }
            });
        },
    );
    changed
}

/// The user's own new tab shortcuts: remove, or add one.
fn shortcuts_editor(ui: &mut egui::Ui, settings: &mut pteron::Settings) -> bool {
    let mut changed = false;
    ui.add_space(6.0);
    ui.label("Your shortcuts:");
    let mut remove = None;
    for (index, shortcut) in settings.look.shortcuts.iter().enumerate() {
        ui.horizontal(|ui| {
            let name = if shortcut.name.is_empty() { shortcut.url.as_str() } else { shortcut.name.as_str() };
            ui.label(name).on_hover_text(&shortcut.url);
            if ui.small_button("Remove").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        settings.look.shortcuts.remove(index);
        changed = true;
    }
    let key = Id::new("pteron_new_shortcut");
    let (mut name, mut address) = ui.data_mut(|d| d.get_temp::<(String, String)>(key)).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.add(TextEdit::singleline(&mut name).hint_text("Name").desired_width(120.0));
        let field = ui.add(TextEdit::singleline(&mut address).hint_text("example.com").desired_width(200.0));
        let entered = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
        if (ui.button("Add").clicked() || entered) && settings.look.add_shortcut(&name, &address) {
            if settings.newtab_tiles == "off" {
                settings.newtab_tiles = "mine".into();
            }
            name.clear();
            address.clear();
            changed = true;
        }
    });
    ui.data_mut(|d| d.insert_temp(key, (name, address)));
    changed
}
