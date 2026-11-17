/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::HashMap;
#[cfg(any(
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos"
))]
use std::fs;
#[cfg(any(
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos"
))]
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use accesskit::Affine;
use dpi::PhysicalSize;
use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{
    Button, FontDefinitions, Id, Key, Label, LayerId, Modifiers, Order, PaintCallback, Panel, Vec2,
    WidgetInfo, WidgetType, pos2,
};
#[cfg(any(
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos"
))]
use egui::{FontData, FontFamily};
use egui_glow::{CallbackFn, EguiGlow};
use egui_winit::EventResponse;
use euclid::{Length, Point2D, Rect, Scale, Size2D};
#[cfg(any(
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos"
))]
use log::info;
use servo::{
    DeviceIndependentPixel, DevicePixel, Image, LoadStatus, OffscreenRenderingContext, PixelFormat,
    RenderingContext, WebView, WebViewId,
};
use url::Url;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::Window;

use crate::desktop::event_loop::AppEvent;
use crate::desktop::{headed_window, settings_page, sidebar, topbar};
use crate::library;
use crate::pteron;
use crate::running_app_state::{RunningAppState, UserInterfaceCommand};
use crate::window::{ServoShellWindow, TopLevelWebViewCreationRequest};

/// The user interface of a headed servoshell. Currently this is implemented via
/// egui.
pub struct Gui {
    rendering_context: Rc<OffscreenRenderingContext>,
    context: EguiGlow,
    toolbar_height: Length<f32, DeviceIndependentPixel>,

    /// Sirocco: Zen-style sidebar instead of the classic top toolbar.
    sidebar_mode: bool,
    /// Sirocco: the large East Asian fonts have been loaded into the interface.
    wide_fonts: bool,
    sidebar: sidebar::State,
    /// Sirocco: the panel open next to the Opera-style side bar.
    rail: topbar::RailPanel,
    /// Sirocco: where the page is drawn, in egui points (everything else is interface).
    page_rect: egui::Rect,
    /// Sirocco: egui's zoom for the screen's scale; the interface size setting multiplies it.
    base_zoom: std::cell::Cell<f32>,
    /// Sirocco: the count of settings changes made outside the settings page that were picked up.
    settings_seen: u64,
    /// Sirocco: the bar between the two pages of a split view (it belongs to the interface).
    split_divider: Option<egui::Rect>,
    /// Sirocco: the window has Sirocco's own top instead of the system's title bar (`None`
    /// until the first frame has set it).
    own_titlebar: Option<bool>,
    /// Sirocco: the system took the mouse over to move the window, so the interface never sees
    /// the button come up and has to be told.
    mouse_taken: bool,
    /// Sirocco: when a page went full screen, and which site (shown for a few seconds, since a
    /// full-screen page hides the address bar and could pretend to be anything).
    fullscreen_notice: Option<(f64, String)>,
    settings: pteron::Settings,
    settings_page: settings_page::PageState,
    /// The tab that was active last frame; switching tabs closes the settings page.
    last_active_tab: Option<WebViewId>,
    /// Sirocco: the system web engine views behind compatibility tabs.
    compat_host: crate::compat_host::Host,

    location: String,

    /// Whether the location has been edited by the user without clicking Go.
    location_dirty: bool,

    /// The [`LoadStatus`] of the active `WebView`.
    load_status: LoadStatus,

    /// The text to display in the status bar on the bottom of the window.
    status_text: Option<String>,

    /// Whether or not the current `WebView` can navigate backward.
    can_go_back: bool,

    /// Whether or not the current `WebView` can navigate forward.
    can_go_forward: bool,

    /// Handle to the GPU texture of the favicon.
    ///
    /// These need to be cached across egui draw calls.
    favicon_textures: HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,

    /// AccessKit tree updates pending the next egui tick.
    /// This allows us to ensure that graft nodes are sent before the subtrees they graft.
    pending_accesskit_updates: Vec<accesskit::TreeUpdate>,
}

fn truncate_with_ellipsis(input: &str, max_length: usize) -> String {
    if input.chars().count() > max_length {
        let truncated: String = input.chars().take(max_length.saturating_sub(1)).collect();
        format!("{}…", truncated)
    } else {
        input.to_string()
    }
}

#[cfg(any(
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos"
))]
fn load_cjk_fonts(font_candidates: &[(&str, &str)]) -> FontDefinitions {
    let mut fonts = interface_fonts();
    let mut loaded_font_names = Vec::new();

    for (path_str, font_name) in font_candidates.iter() {
        let font_path = Path::new(path_str);
        if font_path.exists() {
            match fs::read(font_path) {
                Ok(bytes) => {
                    if !fonts.font_data.contains_key(*font_name) {
                        // Sirocco: these files are 10 to 20 MB each. egui copies font bytes it
                        // owns, so they would be in memory twice; bytes that live for the whole
                        // run are shared instead. They are loaded once and never unloaded.
                        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
                        fonts
                            .font_data
                            .insert(font_name.to_string(), Arc::new(FontData::from_static(bytes)));
                        loaded_font_names.push(font_name.to_string());
                        info!("Loaded font: {}", font_name);
                    }
                },
                Err(error) => {
                    info!("Failed to read font {}: {}", font_name, error);
                },
            }
        }
    }

    // Sirocco: fallbacks behind the interface fonts; the last one loaded is tried first.
    fonts
        .families
        .get_mut(&FontFamily::Proportional)
        .unwrap()
        .extend(loaded_font_names.into_iter().rev());

    fonts
}

/// Sirocco: the fonts the interface starts with. The East Asian fonts used to draw all of its
/// text, but they are tens of MB, so they now load only when something on screen needs them
/// (see `shows_wide_text`). A small system font with the same look takes their place for
/// everything else, so the interface looks the same before and after they load.
#[cfg(any(
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos"
))]
fn interface_fonts() -> FontDefinitions {
    // The interface font, then a font with symbols (✓, ✕, ▲, ▼...) that it lacks.
    #[cfg(target_os = "windows")]
    const CANDIDATES: [&[&str]; 2] = [&[r"C:\Windows\Fonts\segoeui.ttf"], &[r"C:\Windows\Fonts\seguisym.ttf"]];
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    const CANDIDATES: [&[&str]; 2] = [
        &[
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", // Ubuntu/Debian
            "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",   // Fedora
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",          // Arch
            "/usr/local/share/fonts/noto/NotoSans-Regular.ttf",    // FreeBSD
        ],
        &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/local/share/fonts/dejavu/DejaVuSans.ttf",
        ],
    ];
    #[cfg(target_os = "macos")]
    const CANDIDATES: [&[&str]; 2] = [&[], &[]];
    static FONTS: std::sync::OnceLock<[Option<&'static [u8]>; 2]> = std::sync::OnceLock::new();

    let loaded = FONTS.get_or_init(|| {
        CANDIDATES.map(|paths| {
            let bytes = paths.iter().find_map(|path| fs::read(path).ok())?;
            Some(&*Box::leak(bytes.into_boxed_slice()))
        })
    });
    let mut fonts = FontDefinitions::default();
    for (index, (name, bytes)) in ["Interface", "Symbols"].into_iter().zip(loaded).enumerate() {
        let Some(bytes) = bytes else {
            continue;
        };
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
        let family = fonts.families.get_mut(&FontFamily::Proportional).unwrap();
        if index == 0 {
            family.insert(0, name.to_owned());
        } else {
            family.push(name.to_owned());
        }
    }
    fonts
}

#[cfg(target_os = "windows")]
fn configure_fonts() -> FontDefinitions {
    load_cjk_fonts(&[
        (r"C:\Windows\Fonts\malgun.ttf", "Malgun Gothic"), // Korean
        (r"C:\Windows\Fonts\msyh.ttc", "Microsoft YaHei"), // Chinese + Japanese
    ])
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn configure_fonts() -> FontDefinitions {
    load_cjk_fonts(&[
        (
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "Noto Sans CJK",
        ), // Ubuntu/Debian
        (
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "Noto Sans CJK",
        ), // Fedora/Arch
        // FreeBSD splits the Noto CJK fonts into regional subsets
        (
            "/usr/local/share/fonts/noto/NotoSansCJKhk-Regular.otf",
            "Noto Sans CJK HK",
        ),
        (
            "/usr/local/share/fonts/noto/NotoSansCJKjp-Regular.otf",
            "Noto Sans CJK JP",
        ),
        (
            "/usr/local/share/fonts/noto/NotoSansCJKkr-Regular.otf",
            "Noto Sans CJK KR",
        ),
        (
            "/usr/local/share/fonts/noto/NotoSansCJKsc-Regular.otf",
            "Noto Sans CJK SC",
        ),
        (
            "/usr/local/share/fonts/noto/NotoSansCJKtc-Regular.otf",
            "Noto Sans CJK TC",
        ),
        (
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "WenQuanYi Micro Hei",
        ), // common fallback
        (
            "/usr/local/share/fonts/wqy/wqy-microhei.ttc",
            "WenQuanYi Micro Hei",
        ), // FreeBSD
    ])
}

#[cfg(target_os = "macos")]
fn configure_fonts() -> FontDefinitions {
    load_cjk_fonts(&[
        (
            "/System/Library/Fonts/AppleSDGothicNeo.ttc",
            "Apple SD Gothic Neo",
        ), // Korean
        ("/System/Library/Fonts/STHeiti Medium.ttc", "STHeiti Medium"), // Chinese + Japanese
    ])
}

/// Sirocco: Chinese, Japanese or Korean text, which egui's built-in fonts cannot draw.
fn is_wide(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(
            c as u32,
            0x1100..=0x11FF | 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF
        )
    })
}

/// Sirocco: is any text drawn in this frame Chinese, Japanese or Korean? Looks at what egui
/// was asked to draw, so it covers the sidebar, settings, dialogs and messages alike.
fn shows_wide_text(ctx: &egui::Context) -> bool {
    fn wide(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Text(text) => is_wide(text.galley.text()),
            egui::Shape::Vec(shapes) => shapes.iter().any(wide),
            _ => false,
        }
    }
    let mut layers: Vec<LayerId> = ctx.memory(|memory| memory.layer_ids().collect());
    layers.push(LayerId::background());
    ctx.graphics(|graphics| {
        layers
            .iter()
            .filter_map(|layer| graphics.get(*layer))
            .any(|list| list.all_entries().any(|entry| wide(&entry.shape)))
    })
}

impl Drop for Gui {
    fn drop(&mut self) {
        self.rendering_context
            .make_current()
            .expect("Could not make window RenderingContext current");
        self.context.destroy();
    }
}

impl Gui {
    pub(crate) fn new(
        winit_window: &Window,
        event_loop: &ActiveEventLoop,
        event_loop_proxy: EventLoopProxy<AppEvent>,
        rendering_context: Rc<OffscreenRenderingContext>,
        initial_url: Url,
    ) -> Self {
        rendering_context
            .make_current()
            .expect("Could not make window RenderingContext current");
        let mut context = EguiGlow::new(
            event_loop,
            rendering_context.glow_gl_api(),
            None,
            None,
            false,
        );

        // Sirocco: the East Asian fonts are large (tens of MB), so they are loaded only once
        // something on screen needs them (see `shows_wide_text`).
        context.egui_ctx.set_fonts(interface_fonts());
        let sidebar_mode = std::env::var("PTERON_UI").map_or(true, |v| v != "classic");
        // Sirocco: timed frames wait for their time (see `window::repaint_after`).
        context
            .egui_ctx
            .set_request_repaint_callback(|info| crate::window::repaint_after(info.delay));

        context
            .egui_winit
            .init_accesskit(event_loop, winit_window, event_loop_proxy);
        winit_window.set_visible(true);

        context.egui_ctx.options_mut(|options| {
            // Disable the builtin egui handlers for the Ctrl+Plus, Ctrl+Minus and Ctrl+0
            // shortcuts as they don't work well with servoshell's `device-pixel-ratio` CLI argument.
            options.zoom_with_keyboard = false;

            // On platforms where winit fails to obtain a system theme, fall back to a light theme
            // since it is the more common default.
            options.fallback_theme = egui::Theme::Light;
        });
        apply_theme(&context.egui_ctx, &pteron::settings());

        Self {
            rendering_context,
            context,
            toolbar_height: Default::default(),
            sidebar_mode,
            wide_fonts: false,
            sidebar: Default::default(),
            rail: Default::default(),
            page_rect: egui::Rect::NOTHING,
            base_zoom: std::cell::Cell::new(1.0),
            settings_seen: pteron::outside_changes(),
            split_divider: None,
            own_titlebar: None,
            mouse_taken: false,
            fullscreen_notice: None,
            settings: pteron::settings(),
            settings_page: Default::default(),
            last_active_tab: None,
            compat_host: Default::default(),
            location: initial_url.to_string(),
            location_dirty: false,
            load_status: LoadStatus::Complete,
            status_text: None,
            can_go_back: false,
            can_go_forward: false,
            favicon_textures: Default::default(),
            pending_accesskit_updates: vec![],
        }
    }

    pub(crate) fn has_keyboard_focus(&self) -> bool {
        self.context
            .egui_ctx
            .memory(|memory| memory.focused().is_some())
    }

    pub(crate) fn surrender_focus(&self) {
        self.context.egui_ctx.memory_mut(|memory| {
            if let Some(focused) = memory.focused() {
                memory.surrender_focus(focused);
            }
        });
    }

    pub(crate) fn on_window_event(
        &mut self,
        winit_window: &Window,
        event: &WindowEvent,
    ) -> EventResponse {
        self.context.on_window_event(winit_window, event)
    }

    /// The height of the top toolbar of this user inteface ie the distance from the top of the
    /// window to the position of the `WebView`.
    pub(crate) fn toolbar_height(&self) -> Length<f32, DeviceIndependentPixel> {
        self.toolbar_height
    }

    /// Return true iff the given position is over the egui toolbar.
    pub(crate) fn is_in_egui_toolbar_rect(
        &self,
        position: Point2D<f32, DeviceIndependentPixel>,
    ) -> bool {
        // While the settings page is open it covers the whole page area.
        if self.settings_page.open {
            return true;
        }
        if self.sidebar_mode {
            // `position` is in screen points; the interface size setting scales egui's points.
            let scale = self.ui_scale();
            let point = egui::pos2(position.x / scale, position.y / scale);
            let ctx = &self.context.egui_ctx;
            // Menus, lists and pop-ups over the page (not the link address shown at the bottom).
            if ctx
                .layer_id_at(point)
                .is_some_and(|layer| layer.order != Order::Background && layer.id != Id::new("tooltip"))
            {
                return true;
            }
            if !self.page_rect.contains(point) {
                return true;
            }
            // A click beside an open menu closes the menu (and is not for the page).
            if egui::Popup::is_any_open(ctx) {
                return true;
            }
            // The bar between the pages of a split view, also while it is being dragged.
            if self.split_divider.is_some_and(|bar| bar.expand2(egui::vec2(2.0, 0.0)).contains(point)) ||
                ctx.egui_is_using_pointer()
            {
                return true;
            }
            // The edge strip that opens the sidebar (a pinned one is beside the page).
            if self.sidebar.pinned {
                return false;
            }
            let hit = self.sidebar.hit_width;
            return if self.sidebar.right {
                point.x >= self.page_rect.max.x - hit
            } else {
                point.x < self.page_rect.min.x + hit
            };
        }
        position.y < self.toolbar_height.get()
    }

    /// Sirocco: with its own window top the window has no frame to resize it by, so the outer
    /// few points of the window do that. `position` is in screen points.
    pub(crate) fn resize_direction(
        &self,
        position: Point2D<f32, DeviceIndependentPixel>,
    ) -> Option<winit::window::ResizeDirection> {
        use winit::window::ResizeDirection::*;
        if self.own_titlebar != Some(true) {
            return None;
        }
        let size = self.context.egui_ctx.content_rect().size() * self.ui_scale();
        // (Corners reach a little further along the edge.)
        let near = |distance: f32, other: f32| distance < 5.0 || (distance < 14.0 && other < 5.0);
        let (left, top) = (position.x, position.y);
        let (right, bottom) = (size.x - position.x, size.y - position.y);
        let west = near(left, top.min(bottom));
        let east = near(right, top.min(bottom));
        let north = near(top, left.min(right));
        let south = near(bottom, left.min(right));
        Some(match (north, south, west, east) {
            (true, _, true, _) => NorthWest,
            (true, _, _, true) => NorthEast,
            (_, true, true, _) => SouthWest,
            (_, true, _, true) => SouthEast,
            (true, ..) => North,
            (_, true, ..) => South,
            (_, _, true, _) => West,
            (_, _, _, true) => East,
            _ => return None,
        })
    }

    /// Sirocco: true while the settings page is showing.
    pub(crate) fn settings_open(&self) -> bool {
        self.settings_page.open
    }

    /// Sirocco: true when the Zen-style sidebar replaces the classic toolbar.
    pub(crate) fn sidebar_mode(&self) -> bool {
        self.sidebar_mode
    }

    /// Sirocco: open the sidebar and put the cursor in the address bar.
    pub(crate) fn focus_location(&mut self) {
        self.sidebar.focus_location = true;
    }

    /// Create a frameless button with square sizing, as used in the toolbar.
    fn toolbar_button(text: &str) -> egui::Button<'_> {
        egui::Button::new(text)
            .frame(false)
            .min_size(Vec2 { x: 20.0, y: 20.0 })
    }

    /// Draws a browser tab, checking for clicks and queues appropriate [`UserInterfaceCommand`]s.
    /// Using a custom widget here would've been nice, but it doesn't seem as though egui
    /// supports that, so we arrange multiple Widgets in a way that they look connected.
    fn browser_tab(
        ui: &mut egui::Ui,
        window: &ServoShellWindow,
        webview: WebView,
        favicon_texture: Option<egui::load::SizedTexture>,
    ) {
        let label = match (webview.page_title(), webview.url()) {
            (Some(title), _) if !title.is_empty() => title,
            (_, Some(url)) => url.to_string(),
            _ => "New Tab".into(),
        };

        let inactive_bg_color = ui.visuals().window_fill;
        let active_bg_color = ui.visuals().widgets.active.weak_bg_fill;
        let active = window.active_webview().map(|webview| webview.id()) == Some(webview.id());

        // Setup a tab frame that will contain the favicon, title and close button
        let mut tab_frame = egui::Frame::NONE.corner_radius(8).begin(ui);
        {
            tab_frame.content_ui.add_space(5.0);

            let visuals = tab_frame.content_ui.visuals_mut();
            // Remove the stroke so we don't see the border between the close button and the label
            visuals.widgets.active.bg_stroke.width = 0.0;
            visuals.widgets.hovered.bg_stroke.width = 0.0;
            // Now we make sure the fill color is always the same, irrespective of state, that way
            // we can make sure that both the label and close button have the same background color
            visuals.widgets.noninteractive.weak_bg_fill = inactive_bg_color;
            visuals.widgets.inactive.weak_bg_fill = inactive_bg_color;
            visuals.widgets.hovered.weak_bg_fill = active_bg_color;
            visuals.widgets.active.weak_bg_fill = active_bg_color;
            visuals.selection.bg_fill = active_bg_color;
            visuals.selection.stroke.color = visuals.widgets.active.fg_stroke.color;
            visuals.widgets.hovered.fg_stroke.color = visuals.widgets.active.fg_stroke.color;

            // Expansion would also show that they are 2 separate widgets
            visuals.widgets.active.expansion = 0.0;
            visuals.widgets.hovered.expansion = 0.0;

            if let Some(favicon) = favicon_texture {
                tab_frame.content_ui.add(
                    egui::Image::from_texture(favicon)
                        .fit_to_exact_size(egui::vec2(16.0, 16.0))
                        .bg_fill(egui::Color32::TRANSPARENT),
                );
            }

            let tab = tab_frame
                .content_ui
                // No frame: its interact rect is taller than the tab, so only the tab frame is filled.
                .add(Button::selectable(active, truncate_with_ellipsis(&label, 20)).frame(false))
                .on_hover_ui(|ui| {
                    ui.label(&label);
                });

            let close_button = tab_frame
                .content_ui
                .add(egui::Button::new("X").fill(egui::Color32::TRANSPARENT));
            close_button.widget_info(|| {
                let mut info = WidgetInfo::new(WidgetType::Button);
                info.label = Some("Close".into());
                info
            });
            if close_button.clicked() || close_button.middle_clicked() || tab.middle_clicked() {
                window
                    .queue_user_interface_command(UserInterfaceCommand::CloseWebView(webview.id()));
            } else if !active && tab.clicked() {
                window.activate_webview(webview.id());
            }
        }

        let response = tab_frame.allocate_space(ui);
        let fill_color = if active || response.hovered() {
            active_bg_color
        } else {
            inactive_bg_color
        };
        tab_frame.frame.fill = fill_color;
        tab_frame.end(ui);
    }

    /// Update the user interface, but do not paint the updated state.
    pub(crate) fn update(
        &mut self,
        state: &RunningAppState,
        window: &ServoShellWindow,
        headed_window: &headed_window::HeadedWindow,
    ) {
        self.rendering_context
            .make_current()
            .expect("Could not make RenderingContext current");
        let Self {
            rendering_context,
            context,
            toolbar_height,
            location,
            location_dirty,
            favicon_textures,
            sidebar,
            sidebar_mode,
            wide_fonts,
            rail,
            page_rect,
            base_zoom,
            settings_seen,
            split_divider,
            own_titlebar,
            mouse_taken,
            fullscreen_notice,
            settings,
            settings_page,
            last_active_tab,
            compat_host,
            ..
        } = self;

        let winit_window = headed_window.winit_window();
        // Sirocco: the system moved the window with the mouse held down; the button is up now.
        if std::mem::take(mouse_taken) {
            let events = &mut context.egui_winit.egui_input_mut().events;
            events.push(egui::Event::PointerButton {
                pos: egui::Pos2::ZERO,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            });
            events.push(egui::Event::PointerGone);
        }
        // Sirocco: its own window top instead of the system's title bar (a setting).
        let own = *sidebar_mode &&
            !settings.look.system_titlebar &&
            std::env::var_os("PTERON_SYSTEM_TITLEBAR").is_none();
        if *own_titlebar != Some(own) {
            *own_titlebar = Some(own);
            winit_window.set_decorations(!own);
            #[cfg(target_os = "windows")]
            winit::platform::windows::WindowExtWindows::set_undecorated_shadow(winit_window, own);
        }
        context.run(winit_window, |ctx| {
            load_pending_favicons(ctx, window, favicon_textures);

            // Sirocco: settings changed elsewhere (the new tab page's shortcuts).
            let seen = pteron::outside_changes();
            if seen != *settings_seen {
                *settings_seen = seen;
                *settings = pteron::settings();
            }
            // Sirocco: the interface size.
            let scale = if *sidebar_mode { settings.look.scale as f32 / 100.0 } else { 1.0 };
            let zoom = base_zoom.get() * scale;
            if (ctx.zoom_factor() - zoom).abs() > 0.001 {
                ctx.set_zoom_factor(zoom);
            }

            // Sirocco: sites that switched to the compatibility tab by themselves are remembered.
            let keep = crate::compat_view::take_keep();
            if !keep.is_empty() {
                settings.compat_tab_keep_sites.extend(keep);
                settings.compat_tab_keep_sites = crate::compat_view::clean_sites(std::mem::take(&mut settings.compat_tab_keep_sites));
                pteron::update(settings.clone());
            }
            let remembered = crate::compat_view::take_remembered();
            if !remembered.is_empty() {
                settings.compat_tab_sites.extend(remembered);
                settings.compat_tab_sites = crate::compat_view::clean_sites(std::mem::take(&mut settings.compat_tab_sites));
                pteron::update(settings.clone());
            }

            // TODO: While in fullscreen add some way to mitigate the increased phishing risk
            // when not displaying the URL bar: https://github.com/servo/servo/issues/32443
            // Show toolbar unless fullscreen is from document (web API)
            if *sidebar_mode && !headed_window.is_fullscreen_from_document() {
                // Sirocco: the sidebar floats over the page, so the page keeps the full window.
                *toolbar_height = Length::default();
                // A compatibility tab is a native view that egui cannot see the mouse over; the page
                // reports it, so the sidebar can still open at the left edge and close again.
                let on_compat = window
                    .active_webview()
                    .and_then(|webview| webview.url())
                    .is_some_and(|url| crate::compat_view::target(&url).is_some());
                sidebar.page_hover = on_compat && crate::compat_host::pointer_in_page();
                sidebar.page_edge = on_compat && crate::compat_host::pointer_at_edge(settings.look.sidebar_right);
                let look = settings.look.clone();
                sidebar.width = look.sidebar_width as f32;
                sidebar.right = look.sidebar_right;
                sidebar::set_row_style(look.row_height(), look.close_always);
                if look.top_tabs() {
                    sidebar.pinned = false;
                    sidebar.visible = false;
                    sidebar.hit_width = 0.0;
                } else {
                    sidebar.pinned = settings.sidebar_pinned;
                    sidebar::update_visibility(ctx, sidebar);
                }
                let bookmarks_list = library::bookmarks();
                // The page in the active tab, if it is an ordinary web page.
                let current_page: Option<(String, String)> = window
                    .active_webview()
                    .and_then(|webview| {
                        let url = webview.url()?;
                        matches!(url.scheme(), "http" | "https")
                            .then(|| (url.to_string(), webview.page_title().unwrap_or_default()))
                    });
                let security = window
                    .active_webview()
                    .and_then(|webview| webview.url())
                    .map(|url| match url.scheme() {
                        "https" => sidebar::Security::Secure,
                        "http" => {
                            let local = matches!(
                                url.host_str(),
                                Some("localhost") | Some("127.0.0.1") | Some("[::1]")
                            );
                            if local {
                                sidebar::Security::None
                            } else {
                                sidebar::Security::NotSecure
                            }
                        },
                        _ => sidebar::Security::None,
                    })
                    .unwrap_or_default();
                let model = sidebar::Model {
                    security,
                    tabs: window
                        .webviews()
                        .into_iter()
                        .map(|(id, webview)| sidebar::TabRow {
                            label: {
                                match (webview.page_title(), webview.url()) {
                                    (Some(title), _) if !title.is_empty() => title,
                                    (_, Some(url)) => url.to_string(),
                                    _ => "New Tab".into(),
                                }
                            },
                            active: window.active_webview().map(|active| active.id()) == Some(id),
                            pinned: window.is_pinned(id),
                            favicon: favicon_textures
                                .get(&id)
                                .map(|(_, favicon)| *favicon)
                                .filter(|_| look.tab_icons),
                            awake: window.is_awake(id),
                            split: window.split().is_some_and(|(left, right)| left == id || right == id),
                            audible: window.is_audible(id),
                            muted: window.is_muted(id),
                            private: window.is_private(id),
                        })
                        .collect(),
                    sleeping: window
                        .sleeping_tabs()
                        .into_iter()
                        .map(|tab| sidebar::SleepingRow {
                            label: match tab.title.filter(|title| !title.is_empty()) {
                                Some(title) => title,
                                None => tab.url.to_string(),
                            },
                            pinned: tab.pinned,
                        })
                        .collect(),
                    can_go_back: self.can_go_back,
                    can_go_forward: self.can_go_forward,
                    loading: !matches!(self.load_status, LoadStatus::Complete),
                    experimental: state.experimental_preferences_enabled(),
                    bookmarks: bookmarks_list
                        .iter()
                        .map(|bookmark| sidebar::BookmarkRow {
                            label: if bookmark.title.is_empty() {
                                bookmark.url.clone()
                            } else {
                                bookmark.title.clone()
                            },
                            url: bookmark.url.clone(),
                            folder: bookmark.folder.clone(),
                        })
                        .collect(),
                    bookmarked: current_page
                        .as_ref()
                        .is_some_and(|(url, _)| library::is_bookmarked(url)),
                    can_bookmark: current_page.is_some(),
                    suggestions: if *location_dirty &&
                        sidebar::location_focused(ctx) &&
                        !location.trim().is_empty()
                    {
                        let mut list = library::suggest(location, 6);
                        if let Some((title, url)) = pteron::keyword_hint(location) {
                            list.insert(
                                0,
                                library::Suggestion { url, title, bookmarked: false },
                            );
                            list.truncate(6);
                        }
                        list
                    } else {
                        Vec::new()
                    },
                    dark_here: current_page.as_ref().filter(|_| look.dark_pages).and_then(|(url, _)| {
                        Url::parse(url).ok()?.host_str().map(|host| look.dark_for(host))
                    }),
                    own_titlebar: own,
                    private: window.active_webview().is_some_and(|w| window.is_private(w.id())),
                };
                let height = ctx.available_rect_before_wrap().height();
                let actions = if look.top_tabs() {
                    topbar::show(ctx, sidebar, rail, &model, &look, location, location_dirty)
                } else {
                    sidebar::show(ctx, sidebar, &model, &look, location, location_dirty, height)
                };
                let ids: Vec<WebViewId> = window.webviews().into_iter().map(|(id, _)| id).collect();
                for action in actions {
                    // Any tab or navigation action leaves the settings page.
                    if !matches!(
                        action,
                        sidebar::Action::ToggleSettings | sidebar::Action::SetExperimental(_)
                    ) {
                        settings_page.open = false;
                    }
                    match action {
                        sidebar::Action::Back => {
                            window.queue_user_interface_command(UserInterfaceCommand::Back)
                        },
                        sidebar::Action::Forward => {
                            window.queue_user_interface_command(UserInterfaceCommand::Forward)
                        },
                        sidebar::Action::Reload => {
                            window.queue_user_interface_command(UserInterfaceCommand::Reload)
                        },
                        sidebar::Action::Go(text) => {
                            window.queue_user_interface_command(UserInterfaceCommand::Go(text))
                        },
                        sidebar::Action::NewTab => {
                            window.queue_user_interface_command(UserInterfaceCommand::NewWebView)
                        },
                        sidebar::Action::NewPrivateTab => {
                            window.queue_user_interface_command(UserInterfaceCommand::NewPrivateTab)
                        },
                        sidebar::Action::NewWindow => {
                            let url = Url::parse("servo:newtab")
                                .expect("Should be able to unconditionally parse 'servo:newtab' as URL");
                            window.queue_user_interface_command(UserInterfaceCommand::NewWindow(
                                TopLevelWebViewCreationRequest::WithUrl(url),
                            ));
                        },
                        sidebar::Action::Activate(index) => {
                            if let Some(id) = ids.get(index) {
                                window.activate_webview(*id);
                            }
                        },
                        sidebar::Action::Close(index) => {
                            if let Some(id) = ids.get(index) {
                                window.queue_user_interface_command(
                                    UserInterfaceCommand::CloseWebView(*id),
                                );
                            }
                        },
                        sidebar::Action::Wake(index) => {
                            window.queue_user_interface_command(UserInterfaceCommand::WakeTab(index))
                        },
                        sidebar::Action::CloseSleeping(index) => window
                            .queue_user_interface_command(UserInterfaceCommand::CloseSleepingTab(
                                index,
                            )),
                        sidebar::Action::ReaderMode => {
                            if let Some(webview) = window.active_webview() {
                                webview.evaluate_javascript(
                                    crate::reader::script(&pteron::settings()),
                                    |value| {
                                        if let Ok(servo::JSValue::String(text)) = value {
                                            if text == "none" {
                                                crate::reader::set_message(
                                                    "Reading mode: no article found on this page.",
                                                );
                                            }
                                        }
                                    },
                                );
                            }
                        },
                        sidebar::Action::ToggleBookmark => {
                            if let Some((url, title)) = &current_page {
                                library::toggle_bookmark(url, title);
                            }
                        },
                        sidebar::Action::OpenBookmark(index) => {
                            if let Some(bookmark) = bookmarks_list.get(index) {
                                window.queue_user_interface_command(UserInterfaceCommand::Go(
                                    bookmark.url.clone(),
                                ));
                            }
                        },
                        sidebar::Action::OpenBookmarkInNewTab(index) => {
                            if let Some(url) = bookmarks_list
                                .get(index)
                                .and_then(|bookmark| Url::parse(&bookmark.url).ok())
                            {
                                window.queue_user_interface_command(
                                    UserInterfaceCommand::NewTabWithUrl(url),
                                );
                            }
                        },
                        sidebar::Action::RemoveBookmark(index) => library::remove_bookmark(index),
                        sidebar::Action::MoveBookmark(index, folder) => {
                            library::set_bookmark_folder(index, &folder)
                        },
                        sidebar::Action::MoveBookmarkTo { from, to } => library::move_bookmark(from, to),
                        sidebar::Action::MoveTab { from, to } => {
                            if let (Some(from), Some(to)) = (ids.get(from), ids.get(to)) {
                                window.move_tab(*from, *to);
                            }
                        },
                        sidebar::Action::OpenExternal(index) => {
                            if let Some(url) = ids
                                .get(index)
                                .and_then(|id| window.webview_by_id(*id))
                                .and_then(|webview| webview.url())
                            {
                                crate::downloads::open_in_default_browser(url.as_str());
                            }
                        },
                        sidebar::Action::CheckPage(index) => {
                            if let Some(webview) = ids.get(index).and_then(|id| window.webview_by_id(*id)) {
                                webview.evaluate_javascript(crate::sitecheck::report_js(), |_| {});
                            }
                        },
                        sidebar::Action::TogglePin(index) => {
                            if let Some(id) = ids.get(index) {
                                window.queue_user_interface_command(
                                    UserInterfaceCommand::TogglePin(*id),
                                );
                            }
                        },
                        sidebar::Action::TogglePinSleeping(index) => window
                            .queue_user_interface_command(
                                UserInterfaceCommand::TogglePinSleeping(index),
                            ),
                        sidebar::Action::ToggleSettings => {
                            settings_page.open = !settings_page.open;
                        },
                        sidebar::Action::SetExperimental(enabled) => {
                            state.set_experimental_preferences_enabled(enabled);
                            window.queue_user_interface_command(UserInterfaceCommand::ReloadAll);
                        },
                        sidebar::Action::Home => {
                            if let Some(webview) = window.active_webview() {
                                webview.load(Url::parse("servo:newtab").expect("a valid address"));
                            }
                        },
                        sidebar::Action::ToggleDarkSite => {
                            let host = current_page
                                .as_ref()
                                .and_then(|(url, _)| Url::parse(url).ok()?.host_str().map(str::to_string));
                            if let Some(host) = host {
                                settings.look.toggle_dark_site(&host);
                                pteron::update(settings.clone());
                                state.refresh_page_look();
                                window.queue_user_interface_command(UserInterfaceCommand::Reload);
                            }
                        },
                        sidebar::Action::ReloadTab(index) => {
                            if let Some(webview) = ids.get(index).and_then(|id| window.webview_by_id(*id)) {
                                webview.reload();
                            }
                        },
                        sidebar::Action::Duplicate(index) => {
                            if let Some(url) = ids
                                .get(index)
                                .and_then(|id| window.webview_by_id(*id))
                                .and_then(|webview| webview.url())
                            {
                                window.queue_user_interface_command(UserInterfaceCommand::NewTabWithUrl(url));
                            }
                        },
                        sidebar::Action::ToggleAwake(index) => {
                            if let Some(id) = ids.get(index) {
                                window.toggle_awake(*id);
                            }
                        },
                        sidebar::Action::ToggleMute(index) => {
                            if let Some(id) = ids.get(index) {
                                window.toggle_muted(*id);
                            }
                        },
                        sidebar::Action::Stop => window.stop_loading(),
                        sidebar::Action::Screenshot(index) => {
                            if let Some(webview) = ids.get(index).and_then(|id| window.webview_by_id(*id)) {
                                // Only a page on screen is drawn, so show the tab first.
                                window.activate_webview(webview.id());
                                let title = webview.page_title().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "Page".into());
                                webview.take_screenshot(None, move |result| match result {
                                    Ok(image) => {
                                        let _ = std::thread::Builder::new().name("pteron-screenshot".into()).spawn(move || {
                                            let path = crate::downloads::screenshot_path(&title);
                                            crate::downloads::set_notice(
                                                match image::DynamicImage::ImageRgba8(image).save_with_format(&path, image::ImageFormat::Png) {
                                                    Ok(()) => format!("Screenshot saved: {}", path.display()),
                                                    Err(error) => format!("Could not save the screenshot: {error}"),
                                                },
                                            );
                                        });
                                    },
                                    Err(error) => crate::downloads::set_notice(format!("Could not take a screenshot ({error:?}).")),
                                });
                            }
                        },
                        sidebar::Action::History => {
                            if let Ok(url) = Url::parse("servo:history") {
                                window.queue_user_interface_command(UserInterfaceCommand::NewTabWithUrl(url));
                            }
                        },
                        sidebar::Action::Window(sidebar::WindowAction::Drag) => {
                            if winit_window.drag_window().is_ok() {
                                *mouse_taken = true;
                            }
                        },
                        sidebar::Action::Window(sidebar::WindowAction::Minimize) => {
                            winit_window.set_minimized(true)
                        },
                        sidebar::Action::Window(sidebar::WindowAction::Maximize) => {
                            winit_window.set_maximized(!winit_window.is_maximized())
                        },
                        sidebar::Action::Window(sidebar::WindowAction::Close) => {
                            // (The same as the system's close button.)
                            if pteron::settings().clear_on_exit {
                                library::clear_history();
                                state.servo().network_manager().clear_cache();
                            }
                            window.schedule_close();
                        },
                        sidebar::Action::SplitWith(index) => window.set_split_with(ids.get(index).copied()),
                        sidebar::Action::LeaveSplit => window.set_split_with(None),
                        sidebar::Action::CloseOthers(index) | sidebar::Action::CloseAfter(index) => {
                            // Pinned tabs stay; sleeping tabs are listed after the live ones.
                            let after = matches!(action, sidebar::Action::CloseAfter(_));
                            for (at, id) in ids.iter().enumerate() {
                                if at != index && (!after || at > index) && !window.is_pinned(*id) {
                                    window.queue_user_interface_command(UserInterfaceCommand::CloseWebView(*id));
                                }
                            }
                            let sleeping = window.sleeping_tabs();
                            for at in (0..sleeping.len()).rev().filter(|at| !sleeping[*at].pinned) {
                                window.queue_user_interface_command(UserInterfaceCommand::CloseSleepingTab(at));
                            }
                        },
                        sidebar::Action::ToggleBookmarksBar => {
                            settings.look.bookmarks_bar = !settings.look.bookmarks_bar;
                            pteron::update(settings.clone());
                        },
                    }
                }
            } else if !headed_window.is_fullscreen_from_document() {
                let frame = egui::Frame::default()
                    .fill(ctx.style().visuals.window_fill)
                    .inner_margin(4.0);
                Panel::top("toolbar").frame(frame).show_inside(ctx, |ui| {
                    ui.allocate_ui_with_layout(
                        ui.available_size(),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let back_button =
                                ui.add_enabled(self.can_go_back, Gui::toolbar_button("⏴"));
                            back_button.widget_info(|| {
                                let mut info = WidgetInfo::new(WidgetType::Button);
                                info.label = Some("Back".into());
                                info
                            });
                            if back_button.clicked() {
                                *location_dirty = false;
                                window.queue_user_interface_command(UserInterfaceCommand::Back);
                            }

                            let forward_button =
                                ui.add_enabled(self.can_go_forward, Gui::toolbar_button("⏵"));
                            forward_button.widget_info(|| {
                                let mut info = WidgetInfo::new(WidgetType::Button);
                                info.label = Some("Forward".into());
                                info
                            });
                            if forward_button.clicked() {
                                *location_dirty = false;
                                window.queue_user_interface_command(UserInterfaceCommand::Forward);
                            }

                            match self.load_status {
                                LoadStatus::Started | LoadStatus::HeadParsed => {
                                    let stop_button = ui.add(Gui::toolbar_button("X"));
                                    stop_button.widget_info(|| {
                                        let mut info = WidgetInfo::new(WidgetType::Button);
                                        info.label = Some("Stop".into());
                                        info
                                    });
                                    if stop_button.clicked() {
                                        window.stop_loading();
                                    }
                                },
                                LoadStatus::Complete | LoadStatus::Stopped => {
                                    let reload_button = ui.add(Gui::toolbar_button("↻"));
                                    reload_button.widget_info(|| {
                                        let mut info = WidgetInfo::new(WidgetType::Button);
                                        info.label = Some("Reload".into());
                                        info
                                    });
                                    if reload_button.clicked() {
                                        *location_dirty = false;
                                        window.queue_user_interface_command(
                                            UserInterfaceCommand::Reload,
                                        );
                                    }
                                },
                            }
                            ui.add_space(2.0);

                            ui.allocate_ui_with_layout(
                                ui.available_size(),
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let mut experimental_preferences_enabled =
                                        state.experimental_preferences_enabled();
                                    let prefs_toggle = ui
                                        .toggle_value(&mut experimental_preferences_enabled, "☢")
                                        .on_hover_text("Enable experimental prefs");
                                    prefs_toggle.widget_info(|| {
                                        let mut info = WidgetInfo::new(WidgetType::Button);
                                        info.label = Some("Enable experimental preferences".into());
                                        info.selected = Some(experimental_preferences_enabled);
                                        info
                                    });
                                    if prefs_toggle.clicked() {
                                        state.set_experimental_preferences_enabled(
                                            experimental_preferences_enabled,
                                        );
                                        *location_dirty = false;
                                        window.queue_user_interface_command(
                                            UserInterfaceCommand::ReloadAll,
                                        );
                                    }

                                    let location_id = egui::Id::new("location_input");
                                    let location_field = ui.add_sized(
                                        ui.available_size(),
                                        egui::TextEdit::singleline(location)
                                            .id(location_id)
                                            .hint_text("Search or enter address"),
                                    );

                                    if location_field.changed() {
                                        *location_dirty = true;
                                    }
                                    // Handle adddress bar shortcut.
                                    if ui.input(|i| {
                                        if cfg!(target_os = "macos") {
                                            i.clone().consume_key(Modifiers::COMMAND, Key::L)
                                        } else {
                                            i.clone().consume_key(Modifiers::COMMAND, Key::L) ||
                                                i.clone().consume_key(Modifiers::ALT, Key::D)
                                        }
                                    }) {
                                        // The focus request immediately makes gained_focus return true.
                                        location_field.request_focus();
                                    }
                                    // Select address bar text when it's focused (click or shortcut).
                                    if location_field.gained_focus() &&
                                        let Some(mut state) =
                                            TextEditState::load(ui.ctx(), location_id)
                                    {
                                        // Select the whole input.
                                        state.cursor.set_char_range(Some(CCursorRange::two(
                                            CCursor::new(0),
                                            CCursor::new(location.len()),
                                        )));
                                        state.store(ui.ctx(), location_id);
                                    }
                                    // Navigate to address when enter is pressed in the address bar.
                                    if location_field.lost_focus() &&
                                        ui.input(|i| i.clone().key_pressed(Key::Enter))
                                    {
                                        window.queue_user_interface_command(
                                            UserInterfaceCommand::Go(location.clone()),
                                        );
                                    }
                                },
                            );
                        },
                    );
                });

                // A simple Tab header strip
                let outer = Panel::top("tabs").show_inside(ctx, |ui| {
                    // Add scroll for overflowing tabs
                    egui::ScrollArea::horizontal()
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.allocate_ui_with_layout(
                                ui.available_size(),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    for (id, webview) in window.webviews().into_iter() {
                                        let favicon = favicon_textures
                                            .get(&id)
                                            .map(|(_, favicon)| favicon)
                                            .copied();
                                        Self::browser_tab(ui, window, webview, favicon);
                                    }

                                    // Sirocco: tabs that were put to sleep to save memory.
                                    for (index, tab) in
                                        window.sleeping_tabs().into_iter().enumerate()
                                    {
                                        let label = match tab.title.filter(|title| !title.is_empty()) {
                                            Some(title) => title,
                                            None => tab.url.to_string(),
                                        };
                                        let text = egui::RichText::new(format!(
                                            "zZ {}",
                                            truncate_with_ellipsis(&label, 20)
                                        ))
                                        .weak();
                                        let sleeping_tab = ui
                                            .add(egui::Button::new(text).frame(false))
                                            .on_hover_text(format!(
                                                "Sleeping tab (click to wake, middle-click to close)\n{}",
                                                tab.url
                                            ));
                                        if sleeping_tab.middle_clicked() {
                                            window.queue_user_interface_command(
                                                UserInterfaceCommand::CloseSleepingTab(index),
                                            );
                                        } else if sleeping_tab.clicked() {
                                            window.queue_user_interface_command(
                                                UserInterfaceCommand::WakeTab(index),
                                            );
                                        }
                                    }

                                    let new_tab_button = ui.add(Gui::toolbar_button("+"));
                                    new_tab_button.widget_info(|| {
                                        let mut info = WidgetInfo::new(WidgetType::Button);
                                        info.label = Some("New tab".into());
                                        info
                                    });
                                    if new_tab_button.clicked() {
                                        window.queue_user_interface_command(
                                            UserInterfaceCommand::NewWebView,
                                        );
                                    }

                                    let new_window_button = ui.add(Gui::toolbar_button("⊞"));
                                    new_window_button.widget_info(|| {
                                        let mut info = WidgetInfo::new(WidgetType::Button);
                                        info.label = Some("New window".into());
                                        info
                                    });
                                    if new_window_button.clicked() {
                                        let url = Url::parse("servo:newtab").expect("Should be able to unconditionally parse 'servo:newtab' as URL");
                                        window.queue_user_interface_command(
                                            UserInterfaceCommand::NewWindow(TopLevelWebViewCreationRequest::WithUrl(url)),
                                        );
                                    }
                                },
                            );
                        })
                });

                *toolbar_height = Length::new(outer.response.rect.max.y);
            } else {
                *toolbar_height = Length::default();
            }

            // Sirocco: full-window settings page.
            let active_tab = window.active_webview().map(|webview| webview.id());
            if active_tab != *last_active_tab {
                settings_page.open = false;
                *last_active_tab = active_tab;
            }
            // (A pinned sidebar is a panel, so the page area already starts next to it.)
            let page_area = ctx.available_rect_before_wrap();
            // Sirocco: the page starts where the interface ends (in screen points for the window).
            *page_rect = page_area;
            *toolbar_height = Length::new(page_area.min.y * scale);
            sidebar::set_left_inset(page_area.min.x * scale);
            sidebar::set_ui_scale(scale);
            if settings_page::show(ctx, settings_page, settings, page_area) {
                apply_theme(ctx, settings);
                let before = pteron::settings();
                let page_look = |s: &pteron::Settings| (s.look.dark_pages, s.look.dark_skip.clone(), s.look.site_styles.clone());
                let page_look_changed = page_look(&before) != page_look(settings);
                let compat_changed = before.compat != settings.compat;
                let look_changed = pteron::newtab_look_changed(&before, settings);
                pteron::update(settings.clone());
                if before.compat_tab_handoff && !settings.compat_tab_handoff {
                    crate::compat_host::restore_agent(state.servo());
                }
                if look_changed && !compat_changed {
                    window.queue_user_interface_command(UserInterfaceCommand::ReloadNewTabs);
                }
                if compat_changed {
                    state.apply_compat();
                    window.queue_user_interface_command(UserInterfaceCommand::ReloadAll);
                }
                if page_look_changed {
                    state.refresh_page_look();
                }
            }
            if let Some(result) = crate::sync::take_chosen() {
                settings_page.notice = Some(match result {
                    Ok(folder) => {
                        settings.sync_folder = folder;
                        pteron::update(settings.clone());
                        let message = match crate::sync::join() {
                            Ok(message) => message,
                            Err(message) => message,
                        };
                        // Settings may have come from the other computer.
                        *settings = pteron::settings();
                        apply_theme(ctx, settings);
                        message
                    },
                    Err(message) => message,
                });
            }
            if let Some(result) = crate::appearance::take_picked() {
                settings_page.notice = Some(match result {
                    Ok(()) => {
                        settings.newtab_bg = "picture".to_string();
                        pteron::update(settings.clone());
                        "Picture saved. Open a new tab to see it.".to_string()
                    },
                    Err(message) => message,
                });
            }
            for action in std::mem::take(&mut settings_page.pending) {
                settings_page.notice = Some(match action {
                    settings_page::PageAction::ClearCookies => {
                        let manager = state.servo().site_data_manager();
                        manager.clear_cookies(None);
                        let sites = manager.site_data(servo::StorageType::all());
                        let names: Vec<String> = sites.iter().map(|site| site.name()).collect();
                        let names: Vec<&str> = names.iter().map(String::as_str).collect();
                        manager.clear_site_data(&names, servo::StorageType::all());
                        "Cookies and site data cleared. Reload open pages to log out.".to_string()
                    },
                    settings_page::PageAction::ReloadTabs => {
                        window.queue_user_interface_command(UserInterfaceCommand::ReloadAll);
                        "Open tabs reloaded.".to_string()
                    },
                    settings_page::PageAction::ClearCache => {
                        state.servo().network_manager().clear_cache();
                        "Cache cleared.".to_string()
                    },
                    settings_page::PageAction::ClearHistory => {
                        library::clear_history();
                        "Browsing history cleared.".to_string()
                    },
                    settings_page::PageAction::InstallUpdate => match crate::updater::launch_installer() {
                        Ok(()) => {
                            window.schedule_close();
                            "Updating...".to_string()
                        },
                        Err(message) => message,
                    },
                    settings_page::PageAction::SaveReport(note) => match crate::report::save(&note) {
                        Ok(path) => {
                            crate::report::show_in_folder(&path);
                            settings_page.report_note.clear();
                            format!(
                                "Saved: {}. Send this file to whoever gave you Sirocco.",
                                path.display()
                            )
                        },
                        Err(message) => message,
                    },
                    settings_page::PageAction::CopyReport(note) => {
                        ctx.copy_text(crate::report::text(&note));
                        "Copied the details. Paste them into a message.".to_string()
                    },
                    settings_page::PageAction::ExportBookmarks => match library::export_bookmarks() {
                        Ok(path) => {
                            crate::report::show_in_folder(&path);
                            format!("Saved: {}", path.display())
                        },
                        Err(message) => message,
                    },
                    settings_page::PageAction::ImportBookmarks => match library::import_from_downloads() {
                        Ok(0) => "Nothing new: all those bookmarks were already saved.".to_string(),
                        Ok(1) => "Imported 1 bookmark.".to_string(),
                        Ok(count) => format!("Imported {count} bookmarks."),
                        Err(message) => message,
                    },
                    settings_page::PageAction::ChooseSyncFolder => {
                        crate::sync::choose_folder_async();
                        "Choose a folder in the window that opens.".to_string()
                    },
                    settings_page::PageAction::SyncNow => {
                        let message = match crate::sync::sync_now(true) {
                            Ok(message) => message,
                            Err(message) => message,
                        };
                        *settings = pteron::settings();
                        apply_theme(ctx, settings);
                        message
                    },
                    settings_page::PageAction::StopSync => {
                        settings.sync_folder.clear();
                        pteron::update(settings.clone());
                        "Sync is off. The file in the folder was left where it is.".to_string()
                    },
                    settings_page::PageAction::OpenDownloadPage => {
                        if let Ok(url) = Url::parse(&format!("https://github.com/{}/releases/latest", crate::updater::REPO)) {
                            window.queue_user_interface_command(UserInterfaceCommand::NewTabWithUrl(url));
                        }
                        "Opened the download page in a new tab.".to_string()
                    },
                    settings_page::PageAction::ChoosePicture => {
                        crate::appearance::choose_picture_async();
                        "Choose a picture in the window that opens.".to_string()
                    },
                    settings_page::PageAction::OpenKnownIssues => {
                        if let Ok(url) = Url::parse("servo:known-issues") {
                            window.queue_user_interface_command(UserInterfaceCommand::NewTabWithUrl(url));
                        }
                        String::new()
                    },
                    settings_page::PageAction::ForgetSession => {
                        pteron::clear_session();
                        "Saved tabs forgotten (they are saved again from the tabs open now)."
                            .to_string()
                    },
                });
            }

            let scale =
                Scale::<_, DeviceIndependentPixel, DevicePixel>::new(ctx.pixels_per_point());

            headed_window.for_each_active_dialog(window, |dialog| dialog.update(ctx));

            // If the top parts of the GUI changed size, then update the size of the WebView and also
            // the size of its RenderingContext.
            let available_rect = ctx.available_rect_before_wrap();

            // Build a graft node for each WebView.
            let affine = {
                // The grafted WebView tree reports bounds in device pixels relative to the
                // WebView's own origin, so this node supplies the offset of the WebView within
                // the window, and scales it to the same scale as the rest of the nodes in egui's
                // AccessKit tree.
                let scale = (1.0 / window.platform_window().hidpi_scale_factor().get()) as f64;
                let x = available_rect.min.x as f64;
                let y = available_rect.min.y as f64;
                Affine::new([scale, 0.0, 0.0, scale, x, y])
            };
            for (webview_id, webview) in window.webviews() {
                if let Some(tree_id) = webview.accesskit_tree_id() {
                    let id = egui::Id::new(webview_id);
                    ctx.accesskit_node_builder(id, |node| {
                        node.set_tree_id(tree_id);
                        // Only the transform is set: AccessKit consumers exclude graft nodes from
                        // the presented tree, so bounds on this node would never be read.
                        node.set_transform(affine);
                    });
                }
            }
            let size = Size2D::new(available_rect.width(), available_rect.height()) * scale;
            if let Some(webview) = window.active_webview() &&
                size != webview.size()
            {
                // `rect` is sized to just the WebView viewport, which is required by
                // `OffscreenRenderingContext` See:
                // <https://github.com/servo/servo/issues/38369#issuecomment-3138378527>
                webview.resize(PhysicalSize::new(size.width as u32, size.height as u32))
            }

            // Sirocco: split view. Both pages are drawn into the one picture of the page area,
            // each in its own part, with a bar between them that can be dragged.
            let ppp = ctx.pixels_per_point();
            let place = |rect: egui::Rect| {
                let corner = |pos: egui::Pos2| Point2D::new(((pos.x - available_rect.min.x) * ppp).round(), ((pos.y - available_rect.min.y) * ppp).round());
                euclid::Box2D::new(corner(rect.min), corner(rect.max))
            };
            let active_id = window.active_webview().map(|webview| webview.id());
            let mut active_pane = available_rect;
            // Every page on screen (both of a split view) with the part of the window it has.
            let mut shown_panes: Vec<(WebViewId, egui::Rect)> = Vec::new();
            *split_divider = None;
            if let Some((left, right)) = window.shown_split() {
                let x = (available_rect.min.x + available_rect.width() * window.split_ratio()).round();
                let bar = egui::Rect::from_min_max(pos2(x - 3.0, available_rect.min.y), pos2(x + 3.0, available_rect.max.y));
                let panes = [
                    (left, egui::Rect::from_min_max(available_rect.min, bar.left_bottom())),
                    (right, egui::Rect::from_min_max(bar.right_top(), available_rect.max)),
                ];
                for (id, rect) in panes {
                    if let Some(webview) = window.webview_by_id(id) {
                        webview.set_rect(place(rect));
                    }
                    if Some(id) == active_id {
                        active_pane = rect;
                    }
                    shown_panes.push((id, rect));
                }
                window.set_panes(Some(panes.map(|(id, rect)| {
                    (id, (rect.min.x - available_rect.min.x) * ppp, (rect.max.x - available_rect.min.x) * ppp)
                })));
                let response = ctx
                    .interact(bar.expand2(egui::vec2(2.0, 0.0)), Id::new("pteron_split_divider"), egui::Sense::click_and_drag())
                    .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                if response.double_clicked() {
                    window.set_split_ratio(0.5);
                } else if response.dragged() &&
                    let Some(pos) = response.interact_pointer_pos()
                {
                    window.set_split_ratio((pos.x - available_rect.min.x) / available_rect.width());
                }
                *split_divider = Some(bar);
            } else {
                window.set_panes(None);
                if let Some(webview) = window.active_webview() {
                    webview.set_rect(place(available_rect));
                    shown_panes.push((webview.id(), available_rect));
                }
            }
            // (Lists and menus of the active page measure from its own corner.)
            sidebar::set_pane_inset((active_pane.min.x - available_rect.min.x) * sidebar::ui_scale());

            // Sirocco: the compatibility tab's view covers the page area while it is the active tab
            // and nothing from egui (a menu, the Settings page, the address bar list) needs to be on top.
            {
                let active = window.active_webview();
                let active_url = active.as_ref().and_then(|webview| webview.url());
                // Menus, lists and tips open over the page. On Windows and Linux the system's view
                // keeps showing the page with a hole where they are; elsewhere it steps aside meanwhile.
                let popups: Vec<egui::Rect> = ctx.memory(|memory| {
                    let areas = memory.areas();
                    memory
                        .layer_ids()
                        .filter(|layer| matches!(layer.order, Order::Foreground | Order::Tooltip) && areas.is_visible(layer))
                        .filter_map(|layer| memory.area_rect(layer.id))
                        .filter(|rect| rect.intersects(active_pane))
                        .collect()
                });
                let free = !settings_page.open &&
                    (cfg!(any(windows, target_os = "linux")) || popups.is_empty()) &&
                    !sidebar::location_focused(ctx);
                let live: Vec<WebViewId> = window.webview_ids();
                let current = active.as_ref().zip(active_url.as_ref()).map(|(webview, url)| (webview.id(), url));
                // The view fills the window while the sidebar is hidden, so nothing shows next to the
                // page. The page reports the mouse at its left edge, which opens the sidebar, and
                // over the page, which closes it. egui draws a frame only when something happens,
                // and the mouse over a native view is not something it sees, so while the sidebar
                // is not pinned the window is redrawn a few times a second to read those reports.
                // (The page also wakes Sirocco itself when it reports something; this is the fallback,
                // and with the sidebar pinned it only picks up a finished browser check.)
                let on_compat_page = active_url.as_ref().is_some_and(|url| crate::compat_view::target(url).is_some());
                if on_compat_page {
                    ctx.request_repaint_after(std::time::Duration::from_millis(if sidebar.pinned { 500 } else { 100 }));
                }
                // A sidebar floating over the page leaves the rest of it to the view. (In split
                // view each compatibility tab has its own half, active or not.)
                let panes: Vec<(WebViewId, url::Url, egui::Rect)> = if free {
                    shown_panes
                        .iter()
                        .filter_map(|(id, rect)| {
                            let mut area = *rect;
                            if sidebar.visible && !sidebar.pinned {
                                if sidebar.right {
                                    area.max.x = area.max.x.min(ctx.content_rect().max.x - sidebar.width);
                                } else {
                                    area.min.x = area.min.x.max(sidebar.width);
                                }
                            }
                            let url = window.webview_by_id(*id)?.url()?;
                            Some((*id, url, area))
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let corner = f32::from(ctx.global_style().visuals.menu_corner_radius.nw);
                compat_host.sync(winit_window, &live, current, &panes, (&popups, corner), ctx.pixels_per_point(), ctx.wants_keyboard_input());
                crate::compat_host::note_alive(compat_host.alive());
                // The system engine passed a browser check: Servo carries on with the pass.
                if let Some(handoff) = compat_host.take_handoff() {
                    crate::compat_host::apply_handoff(state.servo(), &handoff);
                    if let Some(host) = handoff.page.host_str() {
                        crate::compat_view::mark_handed_off(host);
                        crate::compat_view::note_handoff(host);
                    }
                    compat_host.release(handoff.id);
                    if let Some(webview) = window.webview_by_id(handoff.id) {
                        webview.load(handoff.page);
                    }
                }
                if compat_host.trimming() {
                    ctx.request_repaint_after(std::time::Duration::from_millis(500));
                }
            }

            if let Some(status_text) = &self.status_text {
                egui::Tooltip::always_open(
                    ctx.clone(),
                    LayerId::new(Order::Tooltip, Id::new("tooltip")),
                    "tooltip layer".into(),
                    pos2(0.0, available_rect.max.y),
                )
                .show(|ui| ui.add(Label::new(status_text.clone()).extend()));
            }

            // Sirocco: find in page (Ctrl+F).
            Self::show_find_bar(ctx, window);
            Self::show_zoom_hud(ctx);

            // Sirocco: say which site went full screen, and how to leave.
            let now = ctx.input(|input| input.time);
            if headed_window.is_fullscreen_from_document() {
                let notice = fullscreen_notice.get_or_insert_with(|| {
                    let site = window
                        .active_webview()
                        .and_then(|webview| webview.url())
                        .and_then(|url| url.host_str().map(|host| host.trim_start_matches("www.").to_string()))
                        .unwrap_or_else(|| "This page".to_string());
                    (now, site)
                });
                if now - notice.0 < 4.0 {
                    egui::Area::new(Id::new("pteron_fullscreen_notice"))
                        .order(Order::Tooltip)
                        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 24.0))
                        .show(ctx, |ui| {
                            egui::Frame::popup(ui.style()).inner_margin(egui::vec2(16.0, 10.0)).show(ui, |ui| {
                                ui.label(egui::RichText::new(format!("{} is now full screen. Press Esc to leave.", notice.1)).size(15.0));
                            });
                        });
                    ctx.request_repaint_after(std::time::Duration::from_millis(250));
                }
            } else {
                *fullscreen_notice = None;
            }
            Self::show_password_bars(ctx, window);

            // Sirocco: short messages from reading mode.
            if let Some(text) = crate::reader::message() {
                egui::Area::new(Id::new("reader_notice"))
                    .order(Order::Tooltip)
                    .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -116.0))
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(text);
                        });
                    });
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }

            // Sirocco: tell the user when a pop-up was blocked.
            if let Some(count) = pteron::popup_notice() {
                egui::Area::new(Id::new("popup_blocked_notice"))
                    .order(Order::Tooltip)
                    .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(if count > 1 {
                                format!("Pop-up blocked ({count} so far). Click a link to open windows.")
                            } else {
                                "Pop-up blocked. Click a link to open windows.".to_string()
                            });
                        });
                    });
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }

            // Sirocco: download messages and progress.
            if let Some(text) = crate::downloads::notice() {
                egui::Area::new(Id::new("download_notice"))
                    .order(Order::Tooltip)
                    .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -64.0))
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(text);
                        });
                    });
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
            if crate::downloads::any_active() {
                ctx.request_repaint_after(std::time::Duration::from_millis(300));
            }

            if !*wide_fonts && shows_wide_text(ctx) {
                *wide_fonts = true;
                ctx.set_fonts(configure_fonts());
                ctx.request_repaint();
            }

            window.repaint_webviews();

            if let Some(render_to_parent) = rendering_context.render_to_parent_callback() {
                ctx.layer_painter(LayerId::background()).add(PaintCallback {
                    rect: available_rect,
                    callback: Arc::new(CallbackFn::new(move |info, painter| {
                        let clip = info.viewport_in_pixels();
                        let rect_in_parent = Rect::new(
                            Point2D::new(clip.left_px, clip.from_bottom_px),
                            Size2D::new(clip.width_px, clip.height_px),
                        );
                        render_to_parent(painter.gl(), rect_in_parent)
                    })),
                });
            }
            // Sirocco: split view's bar, and a line around the page that has the keyboard.
            if let Some(bar) = *split_divider {
                let [r, g, b] = crate::appearance::current_accent();
                let painter = ctx.layer_painter(LayerId::background());
                painter.rect_filled(bar, 0.0, ctx.visuals().panel_fill);
                painter.rect_stroke(
                    active_pane,
                    0.0,
                    egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(r, g, b)),
                    egui::StrokeKind::Inside,
                );
            }
        });

        // If any egui widget requested a repaint, also request a repaint for our
        // containing window. This allows egui widget to animate on their own.
        // Sirocco: only when it is due; a later one is woken up for by the event loop.
        if crate::window::take_due_repaint() {
            window.set_needs_repaint();
        }

        let adapter = self
            .context
            .egui_winit
            .accesskit
            .as_mut()
            .expect("guaranteed by Gui::new()");
        for tree_update in self.pending_accesskit_updates.drain(..) {
            adapter.update_if_active(|| tree_update);
        }
    }

    /// Paint the GUI, as of the last update.
    /// Sirocco: the find-in-page bar, top right over the page.
    /// Sirocco: the "Save password?" and "Fill saved login" bars (bottom right over the page).
    fn show_password_bars(ctx: &egui::Context, window: &ServoShellWindow) {
        use std::time::Duration;
        if crate::passwords::working() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let active = window.active_webview();
        let page = active.as_ref().and_then(|webview| webview.url());
        if let (Some(webview), Some(page)) = (&active, &page) &&
            let Some(script) = crate::passwords::take_fill_script(page)
        {
            webview.evaluate_javascript(script, |_| {});
        }
        let place = |id: &'static str| {
            egui::Area::new(Id::new(id))
                .order(Order::Tooltip)
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -176.0))
        };
        if let Some((host, user, replaces)) = crate::passwords::save_prompt() {
            let (mut save, mut later, mut never) = (false, false, false);
            place("pteron_save_password").show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let who = if user.is_empty() { host.clone() } else { format!("{user} on {host}") };
                    ui.label(if replaces {
                        format!("Update the saved password for {who}?")
                    } else {
                        format!("Save the password for {who}?")
                    });
                    ui.horizontal(|ui| {
                        save = ui.button(if replaces { "Update" } else { "Save" }).clicked();
                        later = ui.button("Not now").clicked();
                        never = ui.button("Never for this site").clicked();
                    });
                });
            });
            if save {
                crate::passwords::accept_save();
            } else if never {
                crate::passwords::dismiss_save(true);
            } else if later {
                crate::passwords::dismiss_save(false);
            }
            ctx.request_repaint_after(Duration::from_millis(500));
        } else if let Some(page) = &page &&
            let Some((host, users)) = crate::passwords::fill_offer(page)
        {
            let (mut chosen, mut close) = (None, false);
            place("pteron_fill_password").show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(format!("Saved login for {host}"));
                    for (index, user) in users.iter().enumerate().take(5) {
                        let label = if user.is_empty() { "Fill saved login".to_string() } else { format!("Fill as {user}") };
                        if ui.button(label).clicked() {
                            chosen = Some(index);
                        }
                    }
                    close = ui.button("Not now").clicked();
                });
            });
            if let Some(index) = chosen {
                crate::passwords::request_fill(index);
            } else if close {
                crate::passwords::dismiss_offer();
            }
        } else if let Some(webview) = &active &&
            let Some(page) = &page &&
            matches!(page.scheme(), "http" | "https") &&
            webview
                .page_title()
                .is_some_and(|title| crate::compat_view::is_challenge_title(&title))
        {
            // A "checking your browser" page that Servo cannot pass: Sirocco opens it in the
            // compatibility tab (by itself, or after asking, as the settings say).
            // With the hand-over on, the page only goes to a compatibility tab to pass the check.
            let now = crate::pteron::settings();
            let mode = crate::compat_view::effective_mode(&now).to_string();
            let can = crate::compat_view::usable() && mode != "off";
            let (mut compat, mut open) = (false, false);
            // The check came back on a handed-over site. Right after the hand-over, Servo was not
            // accepted: the site stays in the compatibility tab. Later, the pass ran out: get a new one.
            let returned = page.host_str().and_then(crate::compat_view::check_returned);
            if returned == Some(true) &&
                let Some(host) = page.host_str()
            {
                crate::compat_view::mark_stay(host);
                crate::compat_view::keep_later(host);
            }
            if returned.is_some() {
                compat = can;
            } else if can && mode == "auto" && crate::compat_view::first_switch(&format!("{:?}", webview.id()), page.as_str()) {
                compat = true;
            } else {
                place("pteron_challenge_hint").show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label("This site is checking your browser and Sirocco may not get through.");
                        ui.horizontal(|ui| {
                            if can {
                                compat = ui.button("Open in compatibility tab").clicked();
                            }
                            open = ui.button("Open in default browser").clicked();
                        });
                    });
                });
            }
            if compat && let Some(wrapped) = crate::compat_view::wrap(page) {
                // (With the hand-over on, sites are not remembered: they go to Servo once the check passes.)
                if !crate::pteron::settings().compat_tab_handoff &&
                    let Some(host) = page.host_str()
                {
                    crate::compat_view::remember_later(host);
                }
                webview.load(wrapped);
            }
            if open {
                crate::downloads::open_in_default_browser(page.as_str());
            }
        } else if let Some(webview) = &active &&
            let Some(page) = &page &&
            let Some(host) = page.host_str().and_then(crate::compat_view::handoff_notice) &&
            crate::compat_view::usable()
        {
            // Right after a hand-over to Servo: a way back for pages that Servo draws badly.
            let (mut back, mut fine) = (false, false);
            place("pteron_handoff_hint").show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(format!("{host} continues in Sirocco's own engine."));
                    ui.horizontal(|ui| {
                        back = ui.button("Looks wrong? Use the compatibility tab").clicked();
                        fine = ui.button("OK").clicked();
                    });
                });
            });
            ctx.request_repaint_after(Duration::from_millis(500));
            if back {
                crate::compat_view::mark_stay(&host);
                crate::compat_view::keep_later(&host);
                if let Some(wrapped) = crate::compat_view::wrap(page) {
                    webview.load(wrapped);
                }
            }
            if back || fine {
                crate::compat_view::clear_handoff_notice();
            }
        } else if crate::compat::safe_notice_visible(&crate::pteron::settings().compat) {
            let mut ok = false;
            place("pteron_safe_mode").show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label("Sirocco did not close properly last time, so the newer web features are off for this session.");
                    ok = ui.button("OK").clicked();
                });
            });
            if ok {
                crate::compat::dismiss_safe_notice();
            }
        } else if let Some(text) = crate::passwords::message() {
            place("pteron_password_message").show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(text);
                });
            });
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    /// Sirocco: the zoom level, shown for a moment after a zoom shortcut.
    fn show_zoom_hud(ctx: &egui::Context) {
        let Some(percent) = crate::find::zoom_percent() else { return };
        egui::Area::new(Id::new("pteron_zoom_hud"))
            .order(Order::Foreground)
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(egui::RichText::new(format!("{percent}%")).size(16.0).strong());
                });
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    fn show_find_bar(ctx: &egui::Context, window: &ServoShellWindow) {
        use crate::find;
        let active = window.active_webview();
        let tab_id = active.as_ref().map(|webview| format!("{:?}", webview.id()));
        // Another tab was shown: the bar closes.
        find::with(|state| {
            if state.open && state.tab.is_some() && state.tab != tab_id {
                state.open = false;
            }
        });
        let (open, mut query, result, focus) =
            find::with(|s| (s.open, s.query.clone(), s.result.clone(), std::mem::take(&mut s.focus)));
        let Some(webview) = active else { return };
        if !open {
            return;
        }
        find::with(|s| s.tab = tab_id);
        let input_id = Id::new("pteron_find_input");
        let mut direction: Option<i32> = None;
        let mut close = false;
        let mut changed = false;
        egui::Area::new(Id::new("pteron_find_bar"))
            .order(Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 12.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut query)
                                .id(input_id)
                                .hint_text("Find in page")
                                .desired_width(200.0),
                        );
                        if focus {
                            edit.request_focus();
                        }
                        changed = edit.changed();
                        if edit.has_focus() || edit.lost_focus() {
                            let (enter, shift, escape) = ui.input(|i| {
                                (
                                    i.key_pressed(egui::Key::Enter),
                                    i.modifiers.shift,
                                    i.key_pressed(egui::Key::Escape),
                                )
                            });
                            if enter {
                                direction = Some(if shift { -1 } else { 1 });
                                edit.request_focus();
                            }
                            if escape {
                                close = true;
                            }
                        }
                        let label = if query.is_empty() {
                            String::new()
                        } else if result.is_empty() || result == "0/0" {
                            "No matches".to_string()
                        } else {
                            result.replace('/', " of ")
                        };
                        ui.label(label);
                        if ui.small_button("\u{25B2}").on_hover_text("Previous (Shift+Enter)").clicked() {
                            direction = Some(-1);
                        }
                        if ui.small_button("\u{25BC}").on_hover_text("Next (Enter)").clicked() {
                            direction = Some(1);
                        }
                        if ui.small_button("\u{2715}").on_hover_text("Close (Esc)").clicked() {
                            close = true;
                        }
                    });
                });
            });
        if close {
            // Remove the highlights, then give the keyboard back to the page.
            webview.evaluate_javascript(find::script("", 0), |_| {});
            ctx.memory_mut(|memory| memory.surrender_focus(input_id));
            find::with(|s| {
                s.open = false;
                s.query.clear();
                s.result.clear();
            });
            return;
        }
        let step = if changed { Some(0) } else { direction };
        find::with(|s| s.query = query.clone());
        if let Some(step) = step {
            webview.evaluate_javascript(find::script(&query, step), |value| {
                if let Ok(servo::JSValue::String(text)) = value {
                    find::with(|s| s.result = text);
                }
            });
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    pub(crate) fn paint(&mut self, window: &Window) {
        self.rendering_context
            .make_current()
            .expect("Could not make RenderingContext current");
        self.rendering_context
            .parent_context()
            .prepare_for_rendering();
        self.context.paint(window);
        self.rendering_context.parent_context().present();
    }

    /// Updates the location field from the given [`RunningAppState`], unless the user has started
    /// editing it without clicking Go, returning true iff it has changed (needing an egui update).
    fn update_location_in_toolbar(&mut self, window: &ServoShellWindow) -> bool {
        // User edited without clicking Go?
        if self.location_dirty {
            return false;
        }

        let current_url_string = window
            .active_webview()
            .and_then(|webview| {
                let url = webview.url()?;
                // Sirocco: a compatibility tab shows the page it is on, not its placeholder address.
                if crate::compat_view::target(&url).is_some() {
                    return self
                        .compat_host
                        .current_url(webview.id())
                        .or_else(|| Some(crate::compat_view::display(url.as_str())));
                }
                Some(url.to_string())
            })
            // Sirocco: the new-tab page shows an empty address bar.
            .map(|url| if url == "servo:newtab" { String::new() } else { url });
        match current_url_string {
            Some(location) if location != self.location => {
                self.location = location;
                true
            },
            _ => false,
        }
    }

    fn update_load_status(&mut self, window: &ServoShellWindow) -> bool {
        let state_status = window
            .active_webview()
            .map(|webview| webview.load_status())
            .unwrap_or(LoadStatus::Complete);
        let old_status = std::mem::replace(&mut self.load_status, state_status);
        let status_changed = old_status != self.load_status;

        // When the load status changes, we want the new changes to the URL to start
        // being reflected in the location bar.
        if status_changed {
            self.location_dirty = false;
        }

        status_changed
    }

    fn update_status_text(&mut self, window: &ServoShellWindow) -> bool {
        let state_status = window
            .active_webview()
            .and_then(|webview| webview.status_text());
        let old_status = std::mem::replace(&mut self.status_text, state_status);
        old_status != self.status_text
    }

    fn update_can_go_back_and_forward(&mut self, window: &ServoShellWindow) -> bool {
        let (can_go_back, can_go_forward) = window
            .active_webview()
            .map(|webview| (webview.can_go_back(), webview.can_go_forward()))
            .unwrap_or((false, false));
        let old_can_go_back = std::mem::replace(&mut self.can_go_back, can_go_back);
        let old_can_go_forward = std::mem::replace(&mut self.can_go_forward, can_go_forward);
        old_can_go_back != self.can_go_back || old_can_go_forward != self.can_go_forward
    }

    /// Updates all fields taken from the given [`ServoShellWindow`], such as the location field.
    /// Returns true iff the egui needs an update.
    pub(crate) fn update_webview_data(&mut self, window: &ServoShellWindow) -> bool {
        // Note: We must use the "bitwise OR" (|) operator here instead of "logical OR" (||)
        //       because logical OR would short-circuit if any of the functions return true.
        //       We want to ensure that all functions are called. The "bitwise OR" operator
        //       does not short-circuit.
        self.update_load_status(window) |
            self.update_location_in_toolbar(window) |
            self.update_status_text(window) |
            self.update_can_go_back_and_forward(window)
    }

    /// Returns true if a redraw is required after handling the provided event.
    pub(crate) fn handle_accesskit_event(
        &mut self,
        event: &egui_winit::accesskit_winit::WindowEvent,
    ) -> bool {
        match event {
            egui_winit::accesskit_winit::WindowEvent::InitialTreeRequested => {
                self.context.egui_ctx.enable_accesskit();
                true
            },
            egui_winit::accesskit_winit::WindowEvent::ActionRequested(req) => {
                self.context
                    .egui_winit
                    .on_accesskit_action_request(req.clone());
                true
            },
            egui_winit::accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                self.context.egui_ctx.disable_accesskit();
                false
            },
        }
    }

    pub(crate) fn set_zoom_factor(&self, factor: f32) {
        self.base_zoom.set(factor);
        self.context.egui_ctx.set_zoom_factor(factor * self.ui_scale());
    }

    /// Sirocco: the interface size setting (1.0 = 100%).
    fn ui_scale(&self) -> f32 {
        if self.sidebar_mode { self.settings.look.scale as f32 / 100.0 } else { 1.0 }
    }

    pub(crate) fn notify_accessibility_tree_update(&mut self, tree_update: accesskit::TreeUpdate) {
        self.pending_accesskit_updates.push(tree_update);
    }
}

fn embedder_image_to_egui_image(image: &Image) -> egui::ColorImage {
    let width = image.width as usize;
    let height = image.height as usize;

    match image.format {
        PixelFormat::K8 => egui::ColorImage::from_gray([width, height], image.data()),
        PixelFormat::KA8 => {
            // Convert to rgba
            let (chunks, _) = image.data().as_chunks::<2>();
            let data: Vec<u8> = chunks
                .iter()
                .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
                .collect();
            egui::ColorImage::from_rgba_unmultiplied([width, height], &data)
        },
        PixelFormat::RGB8 => egui::ColorImage::from_rgb([width, height], image.data()),
        PixelFormat::RGBA8 => {
            egui::ColorImage::from_rgba_unmultiplied([width, height], image.data())
        },
        PixelFormat::BGRA8 => {
            // Convert from BGRA to RGBA
            let (chunks, _) = image.data().as_chunks::<4>();
            let data: Vec<u8> = chunks
                .iter()
                .flat_map(|chunk| [chunk[2], chunk[1], chunk[0], chunk[3]])
                .collect();
            egui::ColorImage::from_rgba_unmultiplied([width, height], &data)
        },
    }
}

/// Uploads all favicons that have not yet been processed to the GPU.
/// Sirocco: apply the theme setting ("system", "light" or "dark").
/// egui colours for a full Sirocco theme.
fn visuals_for(p: &crate::appearance::Palette) -> egui::Visuals {
    use egui::Color32;
    let c = |rgb: [u8; 3]| Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    let mixed = |a: [u8; 3], b: [u8; 3], t: f32| {
        let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
        Color32::from_rgb(m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2]))
    };
    let mut v = if p.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let (bg, surface, text, accent) = (c(p.bg), c(p.surface), c(p.text), c(p.accent));
    let line = mixed(p.bg, p.text, 0.16);
    v.panel_fill = bg;
    v.window_fill = bg;
    v.faint_bg_color = surface;
    v.extreme_bg_color = surface;
    v.code_bg_color = surface;
    v.window_stroke = egui::Stroke::new(1.0_f32, line);
    v.selection.bg_fill = accent.gamma_multiply(0.35);
    v.selection.stroke = egui::Stroke::new(1.0_f32, accent);
    v.hyperlink_color = accent;
    v.text_cursor.stroke.color = accent;
    let noninteractive = &mut v.widgets.noninteractive;
    noninteractive.bg_fill = bg;
    noninteractive.weak_bg_fill = bg;
    noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, line);
    noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, text);
    let inactive = &mut v.widgets.inactive;
    inactive.bg_fill = surface;
    inactive.weak_bg_fill = surface;
    inactive.bg_stroke = egui::Stroke::NONE;
    inactive.fg_stroke = egui::Stroke::new(1.0_f32, text);
    let hovered = &mut v.widgets.hovered;
    hovered.bg_fill = mixed(p.surface, p.accent, 0.10);
    hovered.weak_bg_fill = mixed(p.surface, p.accent, 0.10);
    hovered.bg_stroke = egui::Stroke::new(1.0_f32, accent.gamma_multiply(0.7));
    hovered.fg_stroke = egui::Stroke::new(1.5_f32, text);
    let active = &mut v.widgets.active;
    active.bg_fill = mixed(p.surface, p.accent, 0.18);
    active.weak_bg_fill = mixed(p.surface, p.accent, 0.18);
    active.bg_stroke = egui::Stroke::new(1.0_f32, accent);
    active.fg_stroke = egui::Stroke::new(2.0_f32, text);
    let open = &mut v.widgets.open;
    open.bg_fill = mixed(p.surface, p.accent, 0.14);
    open.weak_bg_fill = mixed(p.surface, p.accent, 0.14);
    open.bg_stroke = egui::Stroke::new(1.0_f32, line);
    open.fg_stroke = egui::Stroke::new(1.0_f32, text);
    v
}

fn apply_theme(ctx: &egui::Context, settings: &pteron::Settings) {
    // Start from egui's plain looks every time, so switching away from a colour theme
    // leaves nothing behind.
    ctx.set_visuals_of(egui::Theme::Dark, egui::Visuals::dark());
    ctx.set_visuals_of(egui::Theme::Light, egui::Visuals::light());
    let palette = crate::appearance::palette_for(settings);
    ctx.set_theme(match (&palette, settings.theme.as_str()) {
        (Some(p), _) if p.dark => egui::ThemePreference::Dark,
        (Some(_), _) => egui::ThemePreference::Light,
        (None, "light") => egui::ThemePreference::Light,
        (None, "dark") => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    });
    if let Some(p) = &palette {
        let theme = if p.dark { egui::Theme::Dark } else { egui::Theme::Light };
        ctx.set_visuals_of(theme, visuals_for(p));
        crate::appearance::polish_style(ctx, &settings.look);
        return;
    }
    // The plain looks wear Sirocco's own colours (light, dark, or whichever the system uses).
    ctx.set_visuals_of(egui::Theme::Light, visuals_for(&crate::appearance::SIROCCO_LIGHT));
    ctx.set_visuals_of(egui::Theme::Dark, visuals_for(&crate::appearance::SIROCCO_DARK));
    // The chosen accent tints selections and links.
    let [r, g, b] = crate::appearance::accent_rgb(settings);
    let accent = egui::Color32::from_rgb(r, g, b);
    ctx.all_styles_mut(|style| {
        style.visuals.selection.bg_fill = accent.gamma_multiply(0.35);
        style.visuals.selection.stroke.color = accent;
        style.visuals.hyperlink_color = accent;
        style.visuals.text_cursor.stroke.color = accent;
        style.visuals.widgets.hovered.bg_stroke.color = accent.gamma_multiply(0.7);
    });
    crate::appearance::polish_style(ctx, &settings.look);
}

fn load_pending_favicons(
    ctx: &egui::Context,
    window: &ServoShellWindow,
    texture_cache: &mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
) {
    for id in window.take_pending_favicon_loads() {
        let Some(webview) = window.webview_by_id(id) else {
            continue;
        };
        let Some(favicon) = webview.favicon() else {
            continue;
        };

        let egui_image = embedder_image_to_egui_image(&favicon);
        let handle = ctx.load_texture(format!("favicon-{id:?}"), egui_image, Default::default());
        let texture = egui::load::SizedTexture::new(
            handle.id(),
            egui::vec2(favicon.width as f32, favicon.height as f32),
        );

        // We don't need the handle anymore but we can't drop it either since that would cause
        // the texture to be freed.
        texture_cache.insert(id, (handle, texture));
    }
}
