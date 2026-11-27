#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Cross-platform Material Design 3 egui GUI for kool-cpu (Windows & Linux).

mod icons;
mod settings;
mod tray;
mod updater;

use kool_cpu_backend::native_backend;
use kool_cpu_core::{AutoAction, AutoConfig, AutoController, Backend, Telemetry, BUILTIN_PROFILES};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, Pos2, Rect,
    RichText, ScrollArea, Sense, Shape, Stroke, StrokeKind, Vec2,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

const HISTORY_LEN: usize = 60;
const POLL_SECS: u64 = 2;

/// High-contrast Material 3 palette supporting both Dark and Light themes.
#[derive(Clone, Copy)]
struct Palette {
    is_light: bool,
    surface: Color32,
    surface_container: Color32,
    surface_container_high: Color32,
    hovered_bg: Color32,
    outline: Color32,
    outline_variant: Color32,
    on_surface: Color32,
    on_surface_variant: Color32,
    primary: Color32,
    on_primary: Color32,
    primary_container: Color32,
    on_primary_container: Color32,
    error: Color32,
    error_container: Color32,
    cool: Color32,
    warm: Color32,
    grid_line: Color32,
}

impl Palette {
    fn for_theme(light: bool) -> Self {
        if light {
            Self::light()
        } else {
            Self::dark()
        }
    }

    fn dark() -> Self {
        Self {
            is_light: false,
            surface: Color32::from_rgb(16, 19, 24),
            surface_container: Color32::from_rgb(24, 28, 36),
            surface_container_high: Color32::from_rgb(35, 41, 52),
            hovered_bg: Color32::from_rgb(48, 55, 68),
            outline: Color32::from_rgb(122, 132, 150),
            outline_variant: Color32::from_rgb(54, 61, 74),
            on_surface: Color32::from_rgb(241, 243, 247),
            on_surface_variant: Color32::from_rgb(167, 176, 192),
            primary: Color32::from_rgb(122, 184, 255),
            on_primary: Color32::from_rgb(11, 26, 46),
            primary_container: Color32::from_rgb(22, 58, 95),
            on_primary_container: Color32::from_rgb(216, 235, 255),
            error: Color32::from_rgb(248, 113, 113),
            error_container: Color32::from_rgb(92, 26, 30),
            cool: Color32::from_rgb(110, 231, 183),
            warm: Color32::from_rgb(251, 191, 36),
            grid_line: Color32::from_rgba_unmultiplied(227, 226, 230, 18),
        }
    }

    fn light() -> Self {
        Self {
            is_light: true,
            surface: Color32::from_rgb(245, 247, 251),
            surface_container: Color32::from_rgb(255, 255, 255),
            surface_container_high: Color32::from_rgb(232, 238, 247),
            hovered_bg: Color32::from_rgb(220, 228, 240),
            outline: Color32::from_rgb(100, 116, 139),
            outline_variant: Color32::from_rgb(208, 216, 229),
            on_surface: Color32::from_rgb(17, 24, 39),
            on_surface_variant: Color32::from_rgb(71, 85, 105),
            primary: Color32::from_rgb(26, 102, 204),
            on_primary: Color32::from_rgb(255, 255, 255),
            primary_container: Color32::from_rgb(214, 232, 255),
            on_primary_container: Color32::from_rgb(11, 53, 110),
            error: Color32::from_rgb(197, 34, 31),
            error_container: Color32::from_rgb(253, 232, 232),
            cool: Color32::from_rgb(15, 138, 95),
            warm: Color32::from_rgb(180, 83, 9),
            grid_line: Color32::from_rgba_unmultiplied(17, 24, 39, 22),
        }
    }

    fn temp_color(&self, c: f32) -> Color32 {
        if c >= 80.0 {
            self.error
        } else if c >= 70.0 {
            self.warm
        } else {
            self.cool
        }
    }
}

struct SharedState {
    telemetry: Option<Telemetry>,
    history: VecDeque<f32>,
    auto_enabled: bool,
    auto_cfg: AutoConfig,
    pending_cap: Option<Option<u32>>,
    applying: bool,
    status: String,
    is_error: bool,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            telemetry: None,
            history: VecDeque::with_capacity(HISTORY_LEN),
            auto_enabled: false,
            auto_cfg: AutoConfig::default(),
            pending_cap: None,
            applying: false,
            status: "Connecting to sensors...".into(),
            is_error: false,
        }
    }
}

struct WorkerSync {
    state: Mutex<SharedState>,
    wake: Condvar,
}

impl WorkerSync {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(SharedState::default()),
            wake: Condvar::new(),
        })
    }

    fn request_cap(&self, cap: Option<u32>, disable_auto: bool) {
        let mut s = self.state.lock().unwrap();
        if disable_auto {
            s.auto_enabled = false;
        }
        s.pending_cap = Some(cap);
        s.applying = true;
        s.status = match cap {
            Some(m) => format!("Applying {:.2} GHz cap...", m as f32 / 1000.0),
            None => "Removing frequency cap...".into(),
        };
        s.is_error = false;
        self.wake.notify_one();
    }

    fn notify(&self) {
        self.wake.notify_one();
    }
}

/// Turn raw OS/HRESULT errors into actionable recovery copy.
fn friendly_error(raw: &str) -> String {
    if raw.contains("0x80070005") || raw.contains("Access denied") || raw.contains("Administrator") {
        "Administrator privileges required — right-click kool-cpu-gui.exe and Run as administrator".into()
    } else if raw.contains("PawnIO not installed") {
        "PawnIO driver not found — install from https://pawnio.eu/ for live CPU temps".into()
    } else {
        raw.to_string()
    }
}

fn spawn_worker(sync: Arc<WorkerSync>, ctx: egui::Context) {
    thread::spawn(move || {
        let backend: Box<dyn Backend> = native_backend();
        let mut controller: Option<AutoController> = None;
        let mut last_cfg = AutoConfig::default();

        loop {
            let requested = sync.state.lock().unwrap().pending_cap.take();
            if let Some(cap) = requested {
                let res = backend.set_cap_mhz(cap);
                let mut s = sync.state.lock().unwrap();
                s.applying = false;
                match res {
                    Ok(()) => {
                        s.status = match cap {
                            Some(m) => format!("Capped at {:.2} GHz ({m} MHz)", m as f32 / 1000.0),
                            None => "Cap removed — running at full speed".into(),
                        };
                        s.is_error = false;
                    }
                    Err(e) => {
                        s.status = friendly_error(&e);
                        s.is_error = true;
                    }
                }
                controller = None;
            }

            match backend.telemetry() {
                Ok(t) => {
                    let (auto_on, cfg) = {
                        let mut s = sync.state.lock().unwrap();
                        if let Some(c) = t.temp_c {
                            if s.history.len() == HISTORY_LEN {
                                s.history.pop_front();
                            }
                            s.history.push_back(c);
                            if s.status == "Connecting to sensors..." {
                                s.status = format!("Reading {}", t.temp_source);
                                s.is_error = false;
                            }
                        } else if s.status == "Connecting to sensors..." {
                            s.status = friendly_error(&t.temp_source);
                            s.is_error = true;
                        }
                        s.telemetry = Some(t.clone());
                        (s.auto_enabled, s.auto_cfg)
                    };

                    if auto_on {
                        if controller.is_none() || cfg != last_cfg {
                            match AutoController::new(cfg, t.cap_mhz) {
                                Ok(c) => {
                                    controller = Some(c);
                                    last_cfg = cfg;
                                }
                                Err(e) => {
                                    let mut s = sync.state.lock().unwrap();
                                    s.status = format!("Auto config: {e}");
                                    s.is_error = true;
                                    controller = None;
                                }
                            }
                        }
                        if let Some(ctrl) = controller.as_mut() {
                            match ctrl.tick(backend.as_ref(), t.temp_c) {
                                Ok(AutoAction::Throttled(m)) => {
                                    let mut s = sync.state.lock().unwrap();
                                    s.status = format!("Auto mode lowered cap to {:.2} GHz", m as f32 / 1000.0);
                                    s.is_error = false;
                                }
                                Ok(AutoAction::Relaxed(c)) => {
                                    let label = c.map_or("full speed".into(), |m| format!("{:.2} GHz", m as f32 / 1000.0));
                                    let mut s = sync.state.lock().unwrap();
                                    s.status = format!("Auto mode raised cap to {label}");
                                    s.is_error = false;
                                }
                                Ok(AutoAction::NoTemp) => {
                                    let mut s = sync.state.lock().unwrap();
                                    s.status = friendly_error(&t.temp_source);
                                    s.is_error = true;
                                }
                                Ok(AutoAction::Hold) => {}
                                Err(e) => {
                                    let mut s = sync.state.lock().unwrap();
                                    s.status = friendly_error(&e);
                                    s.is_error = true;
                                }
                            }
                        }
                    } else {
                        controller = None;
                    }
                }
                Err(e) => {
                    let mut s = sync.state.lock().unwrap();
                    s.status = friendly_error(&e);
                    s.is_error = true;
                }
            }

            ctx.request_repaint();

            let guard = sync.state.lock().unwrap();
            if guard.pending_cap.is_none() {
                let _ = sync.wake.wait_timeout(guard, Duration::from_secs(POLL_SECS)).unwrap();
            }
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Monitor,
    Settings,
}

struct CoolerApp {
    sync: Arc<WorkerSync>,
    tray: Option<tray::Tray>,
    settings: settings::AppSettings,
    update_state: Arc<Mutex<updater::UpdateState>>,
    relaunch_ready: Arc<AtomicBool>,
    page: Page,
    quitting: bool,
    slider_mhz: u32,
    slider_synced: bool,
    slider_dirty: bool,
}

impl CoolerApp {
    fn new(cc: &eframe::CreationContext<'_>, settings: settings::AppSettings, launched_at_startup: bool) -> Self {
        let pal = Palette::for_theme(settings.light_theme);
        configure_material3(&cc.egui_ctx, &pal);
        let sync = WorkerSync::new();

        // Restore saved Auto config and apply remembered speed (Balanced 3100 MHz by default).
        {
            let mut s = sync.state.lock().unwrap();
            s.auto_cfg = settings.auto_cfg;
            s.auto_enabled = !launched_at_startup && settings.last_auto_enabled;
        }
        if launched_at_startup || !settings.last_auto_enabled {
            sync.request_cap(settings.last_cap_mhz, launched_at_startup);
        }

        spawn_worker(Arc::clone(&sync), cc.egui_ctx.clone());
        let tray = if settings.show_in_tray {
            tray::Tray::new().ok()
        } else {
            None
        };
        let update_state = Arc::new(Mutex::new(updater::UpdateState::Idle));
        let slider_mhz = settings.last_cap_mhz.unwrap_or(settings::DEFAULT_CAP_MHZ);
        let app = Self {
            sync,
            tray,
            settings,
            update_state,
            relaunch_ready: Arc::new(AtomicBool::new(false)),
            page: Page::Monitor,
            quitting: false,
            slider_mhz,
            slider_synced: true,
            slider_dirty: false,
        };
        app.spawn_update_check(cc.egui_ctx.clone(), true);
        app
    }

    fn spawn_update_check(&self, ctx: egui::Context, silent: bool) {
        let state = Arc::clone(&self.update_state);
        *state.lock().unwrap() = updater::UpdateState::Checking;
        thread::spawn(move || {
            let next = match updater::check_latest() {
                Ok(Some(info)) => updater::UpdateState::Available(info),
                Ok(None) => updater::UpdateState::UpToDate,
                Err(_) if silent => updater::UpdateState::Idle,
                Err(e) => updater::UpdateState::Error(e),
            };
            *state.lock().unwrap() = next;
            ctx.request_repaint();
        });
    }

    fn spawn_update_install(&self, info: updater::ReleaseInfo, ctx: egui::Context) {
        let state = Arc::clone(&self.update_state);
        let relaunch = Arc::clone(&self.relaunch_ready);
        let sync = Arc::clone(&self.sync);
        *state.lock().unwrap() = updater::UpdateState::Updating;
        {
            let mut s = sync.state.lock().unwrap();
            s.status = format!("Downloading and verifying {}...", info.tag);
            s.is_error = false;
        }
        thread::spawn(move || {
            match updater::download_and_install(&info) {
                Ok(()) => {
                    relaunch.store(true, Ordering::SeqCst);
                }
                Err(e) => {
                    *state.lock().unwrap() = updater::UpdateState::Error(e.clone());
                    let mut s = sync.state.lock().unwrap();
                    s.status = format!("Update failed: {e}");
                    s.is_error = true;
                }
            }
            ctx.request_repaint();
        });
    }

    fn set_status(&self, msg: impl Into<String>, is_error: bool) {
        let mut s = self.sync.state.lock().unwrap();
        s.status = msg.into();
        s.is_error = is_error;
    }

    fn render_settings(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.label(RichText::new("Settings").size(16.0).strong());
        ui.add_space(4.0);

        let card_stroke = if pal.is_light {
            Stroke::new(1.0, pal.outline_variant)
        } else {
            Stroke::NONE
        };

        Frame::new()
            .fill(pal.surface_container)
            .stroke(card_stroke)
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                // Light theme toggle
                ui.horizontal(|ui| {
                    md_icon_badge(ui, MdIcon::Sun, pal);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new("Light theme").size(13.5).strong());
                        ui.label(
                            RichText::new("Switch between Material Light and Dark appearance")
                                .size(12.0)
                                .color(pal.on_surface_variant),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if md_switch(ui, &mut self.settings.light_theme, pal).changed() {
                            let new_pal = Palette::for_theme(self.settings.light_theme);
                            configure_material3(ui.ctx(), &new_pal);
                            if let Err(e) = self.settings.save() {
                                self.set_status(e, true);
                            } else {
                                let msg = if self.settings.light_theme {
                                    "Light theme enabled"
                                } else {
                                    "Dark theme enabled"
                                };
                                self.set_status(msg, false);
                            }
                        }
                    });
                });

                section_divider(ui, pal);

                // Show in tray
                ui.horizontal(|ui| {
                    md_icon_badge(ui, MdIcon::Tray, pal);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new("Show in system tray").size(13.5).strong());
                        ui.label(
                            RichText::new("Keep kool-cpu in the tray when you close the window")
                                .size(12.0)
                                .color(pal.on_surface_variant),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if md_switch(ui, &mut self.settings.show_in_tray, pal).changed() {
                            if self.settings.show_in_tray {
                                match tray::Tray::new() {
                                    Ok(t) => self.tray = Some(t),
                                    Err(e) => {
                                        self.settings.show_in_tray = false;
                                        self.set_status(format!("Could not create tray icon: {e}"), true);
                                    }
                                }
                            } else {
                                self.tray = None;
                            }
                            if let Err(e) = self.settings.save() {
                                self.set_status(e, true);
                            } else {
                                let msg = if self.settings.show_in_tray {
                                    "System tray enabled"
                                } else {
                                    "System tray disabled — closing window will exit"
                                };
                                self.set_status(msg, false);
                            }
                        }
                    });
                });

                section_divider(ui, pal);

                // Run on startup
                ui.horizontal(|ui| {
                    md_icon_badge(ui, MdIcon::Power, pal);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new("Run on startup").size(13.5).strong());
                        ui.label(
                            RichText::new("Launch at sign-in with your saved speed (default: Balanced 3.1 GHz)")
                                .size(12.0)
                                .color(pal.on_surface_variant),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if md_switch(ui, &mut self.settings.run_on_startup, pal).changed() {
                            match settings::apply_startup(self.settings.run_on_startup) {
                                Ok(()) => {
                                    if let Err(e) = self.settings.save() {
                                        self.set_status(e, true);
                                    } else {
                                        let msg = if self.settings.run_on_startup {
                                            "Startup enabled (uses your saved speed)"
                                        } else {
                                            "Startup disabled"
                                        };
                                        self.set_status(msg, false);
                                    }
                                }
                                Err(e) => {
                                    self.settings.run_on_startup = !self.settings.run_on_startup;
                                    self.set_status(friendly_error(&e), true);
                                }
                            }
                        }
                    });
                });

                section_divider(ui, pal);

                // Reset throttle on exit
                ui.horizontal(|ui| {
                    md_icon_badge(ui, MdIcon::Reset, pal);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new("Reset CPU speed on exit").size(13.5).strong());
                        ui.label(
                            RichText::new("Restore full / default speed when kool-cpu closes or quits")
                                .size(12.0)
                                .color(pal.on_surface_variant),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if md_switch(ui, &mut self.settings.reset_on_exit, pal).changed() {
                            if let Err(e) = self.settings.save() {
                                self.set_status(e, true);
                            } else {
                                let msg = if self.settings.reset_on_exit {
                                    "CPU throttle will reset to full speed on exit"
                                } else {
                                    "CPU cap will persist after exit"
                                };
                                self.set_status(msg, false);
                            }
                        }
                    });
                });
            });

        ui.add_space(10.0);

        // Updates card
        let upd = self.update_state.lock().unwrap().clone();
        let mut start_check = false;
        let mut start_install: Option<updater::ReleaseInfo> = None;

        Frame::new()
            .fill(pal.surface_container)
            .stroke(card_stroke)
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    md_icon_badge(ui, MdIcon::Download, pal);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Updates").size(13.5).strong());
                            ui.label(
                                RichText::new(format!("v{}", updater::CURRENT_VERSION))
                                    .font(FontId::monospace(11.5))
                                    .color(pal.on_surface_variant),
                            );
                        });
                        let (sub, col) = match &upd {
                            updater::UpdateState::Idle => ("Check GitHub Releases for new versions".into(), pal.on_surface_variant),
                            updater::UpdateState::Checking => ("Checking for updates...".into(), pal.on_surface_variant),
                            updater::UpdateState::UpToDate => ("You are on the latest version".into(), pal.cool),
                            updater::UpdateState::Available(info) => (format!("{} is available", info.tag), pal.primary),
                            updater::UpdateState::Updating => ("Downloading & verifying SHA-256...".into(), pal.primary),
                            updater::UpdateState::Error(e) => (e.clone(), pal.error),
                        };
                        ui.label(RichText::new(sub).size(12.0).color(col));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match &upd {
                        updater::UpdateState::Available(info) => {
                            let btn = egui::Button::new(RichText::new("Update").size(12.5).strong().color(pal.on_primary))
                                .fill(pal.primary)
                                .corner_radius(CornerRadius::same(18))
                                .min_size(Vec2::new(76.0, 32.0));
                            if ui.add(btn).clicked() {
                                start_install = Some(info.clone());
                            }
                        }
                        updater::UpdateState::Checking | updater::UpdateState::Updating => {
                            ui.add_enabled(
                                false,
                                egui::Button::new(RichText::new("Working...").size(12.0).color(pal.on_surface_variant))
                                    .fill(pal.surface_container_high)
                                    .corner_radius(CornerRadius::same(18))
                                    .min_size(Vec2::new(76.0, 32.0)),
                            );
                        }
                        _ => {
                            let btn = egui::Button::new(RichText::new("Check").size(12.5).strong().color(pal.on_surface))
                                .fill(pal.surface_container_high)
                                .corner_radius(CornerRadius::same(18))
                                .min_size(Vec2::new(72.0, 32.0));
                            if ui.add(btn).clicked() {
                                start_check = true;
                            }
                        }
                    });
                });
            });

        if start_check {
            self.spawn_update_check(ui.ctx().clone(), false);
        } else if let Some(info) = start_install {
            self.spawn_update_install(info, ui.ctx().clone());
        }

        ui.add_space(10.0);
        ui.label(
            RichText::new(format!("Saved to {}", settings::AppSettings::config_path().display()))
                .size(11.0)
                .color(pal.on_surface_variant),
        );
    }
}

fn configure_material3(ctx: &egui::Context, pal: &Palette) {
    let theme = if pal.is_light { egui::Theme::Light } else { egui::Theme::Dark };
    let mut visuals = if pal.is_light { egui::Visuals::light() } else { egui::Visuals::dark() };
    visuals.panel_fill = pal.surface;
    visuals.window_fill = pal.surface;
    visuals.override_text_color = Some(pal.on_surface);
    visuals.widgets.noninteractive.bg_fill = pal.surface_container;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, pal.on_surface);
    visuals.widgets.inactive.bg_fill = pal.surface_container_high;
    visuals.widgets.inactive.weak_bg_fill = pal.surface_container_high;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, pal.on_surface);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(18);
    visuals.widgets.hovered.bg_fill = pal.hovered_bg;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, pal.on_surface);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, pal.outline_variant);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(18);
    // NOTE: egui uses `widgets.active.fg_stroke.color` for `RichText::strong()`, so keep it high-contrast!
    visuals.widgets.active.bg_fill = pal.primary_container;
    visuals.widgets.active.fg_stroke = Stroke::new(1.5, pal.on_surface);
    visuals.widgets.active.corner_radius = CornerRadius::same(18);
    visuals.selection.bg_fill = pal.primary;
    visuals.selection.stroke = Stroke::new(1.5, pal.primary);
    visuals.slider_trailing_fill = true;

    let mut style = (*ctx.style_of(theme)).clone();
    style.visuals = visuals;
    style.spacing.item_spacing = Vec2::new(8.0, 10.0);
    style.spacing.button_padding = Vec2::new(14.0, 8.0);
    style.spacing.slider_rail_height = 6.0;
    ctx.set_style_of(theme, style);
    ctx.set_theme(theme);
}

fn md_chip(ui: &mut egui::Ui, text: &str, bg: Color32, fg: Color32) {
    Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin::symmetric(12, 5))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).strong().color(fg));
        });
}

/// Drawn Material Symbols vector icons (consistent 1.8px stroke, crisp at any DPI).
#[derive(Clone, Copy)]
enum MdIcon {
    Settings,
    ArrowBack,
    Sun,
    Tray,
    Power,
    Reset,
    Download,
}

fn paint_md_icon(painter: &egui::Painter, center: Pos2, icon: MdIcon, color: Color32) {
    let stroke = Stroke::new(1.8, color);
    match icon {
        MdIcon::Settings => {
            painter.circle_stroke(center, 2.8, stroke);
            painter.circle_stroke(center, 6.2, stroke);
            for i in 0..8 {
                let a = (i as f32) * std::f32::consts::FRAC_PI_4;
                let (sin, cos) = a.sin_cos();
                let p0 = Pos2::new(center.x + cos * 6.4, center.y + sin * 6.4);
                let p1 = Pos2::new(center.x + cos * 8.6, center.y + sin * 8.6);
                painter.line_segment([p0, p1], Stroke::new(2.2, color));
            }
        }
        MdIcon::ArrowBack => {
            let left = Pos2::new(center.x - 6.0, center.y);
            let right = Pos2::new(center.x + 6.0, center.y);
            painter.line_segment([left, right], stroke);
            painter.line_segment([left, Pos2::new(center.x - 1.0, center.y - 5.0)], stroke);
            painter.line_segment([left, Pos2::new(center.x - 1.0, center.y + 5.0)], stroke);
        }
        MdIcon::Sun => {
            painter.circle_stroke(center, 3.6, stroke);
            for i in 0..8 {
                let a = (i as f32) * std::f32::consts::FRAC_PI_4;
                let (sin, cos) = a.sin_cos();
                let p0 = Pos2::new(center.x + cos * 5.8, center.y + sin * 5.8);
                let p1 = Pos2::new(center.x + cos * 8.0, center.y + sin * 8.0);
                painter.line_segment([p0, p1], Stroke::new(1.6, color));
            }
        }
        MdIcon::Tray => {
            let r = Rect::from_center_size(center, Vec2::new(14.0, 12.0));
            painter.rect_stroke(r, 2.5, stroke, StrokeKind::Inside);
            painter.line_segment(
                [Pos2::new(r.left() + 2.0, r.bottom() - 3.5), Pos2::new(r.right() - 2.0, r.bottom() - 3.5)],
                Stroke::new(1.5, color),
            );
        }
        MdIcon::Power => {
            let segs = 18;
            let rad = 6.5;
            let start = -std::f32::consts::FRAC_PI_2 + 0.55;
            let end = std::f32::consts::TAU - std::f32::consts::FRAC_PI_2 - 0.55;
            let pts: Vec<Pos2> = (0..=segs)
                .map(|i| {
                    let a = start + (end - start) * (i as f32 / segs as f32);
                    Pos2::new(center.x + a.cos() * rad, center.y + a.sin() * rad)
                })
                .collect();
            painter.add(Shape::line(pts, stroke));
            painter.line_segment(
                [Pos2::new(center.x, center.y - 8.0), Pos2::new(center.x, center.y - 1.5)],
                stroke,
            );
        }
        MdIcon::Reset => {
            let segs = 18;
            let rad = 6.5;
            let start = -std::f32::consts::FRAC_PI_2;
            let end = start + std::f32::consts::TAU * 0.78;
            let pts: Vec<Pos2> = (0..=segs)
                .map(|i| {
                    let a = start + (end - start) * (i as f32 / segs as f32);
                    Pos2::new(center.x + a.cos() * rad, center.y + a.sin() * rad)
                })
                .collect();
            painter.add(Shape::line(pts, stroke));
            let tip = Pos2::new(center.x, center.y - rad);
            painter.line_segment([tip, Pos2::new(tip.x - 3.5, tip.y - 2.5)], stroke);
            painter.line_segment([tip, Pos2::new(tip.x - 3.0, tip.y + 3.0)], stroke);
        }
        MdIcon::Download => {
            let top = Pos2::new(center.x, center.y - 6.5);
            let tip = Pos2::new(center.x, center.y + 2.5);
            painter.line_segment([top, tip], stroke);
            painter.line_segment([tip, Pos2::new(center.x - 4.0, center.y - 1.5)], stroke);
            painter.line_segment([tip, Pos2::new(center.x + 4.0, center.y - 1.5)], stroke);
            painter.line_segment(
                [Pos2::new(center.x - 6.0, center.y + 6.0), Pos2::new(center.x + 6.0, center.y + 6.0)],
                stroke,
            );
        }
    }
}

/// Material 3 circular 36x36 IconButton with hover/focus states and optional notification dot.
fn md_icon_button(ui: &mut egui::Ui, icon: MdIcon, tooltip: &str, badge_dot: bool, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
    let painter = ui.painter();
    let radius = rect.width() * 0.5;
    let fill = if resp.is_pointer_button_down_on() {
        pal.primary_container
    } else if resp.hovered() {
        pal.surface_container_high
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        painter.circle_filled(rect.center(), radius, fill);
    }
    if resp.has_focus() {
        painter.circle_stroke(rect.center(), radius, Stroke::new(1.5, pal.primary));
    }
    let fg = if resp.hovered() { pal.on_surface } else { pal.on_surface_variant };
    paint_md_icon(painter, rect.center(), icon, fg);
    if badge_dot {
        painter.circle_filled(Pos2::new(rect.right() - 8.0, rect.top() + 8.0), 4.0, pal.primary);
    }
    resp.on_hover_text(tooltip)
}

fn md_icon_badge(ui: &mut egui::Ui, icon: MdIcon, pal: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), 16.0, pal.surface_container_high);
    paint_md_icon(painter, rect.center(), icon, pal.primary);
}

/// Drawn vector checkmark (avoids Unicode glyph substitution).
fn paint_checkmark(painter: &egui::Painter, center: Pos2, color: Color32) {
    let p1 = Pos2::new(center.x - 4.5, center.y + 0.5);
    let p2 = Pos2::new(center.x - 1.2, center.y + 3.5);
    let p3 = Pos2::new(center.x + 4.8, center.y - 3.5);
    let stroke = Stroke::new(1.8, color);
    painter.line_segment([p1, p2], stroke);
    painter.line_segment([p2, p3], stroke);
}

/// Material 3 pill toggle switch with smooth animated thumb and focus ring.
fn md_switch(ui: &mut egui::Ui, on: &mut bool, pal: &Palette) -> egui::Response {
    let size = Vec2::new(48.0, 28.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let how_on = ui.ctx().animate_bool_responsive(resp.id, *on);
    let painter = ui.painter();
    let radius = rect.height() * 0.5;

    if resp.has_focus() {
        painter.rect_stroke(rect.expand(2.5), radius + 2.5, Stroke::new(1.5, pal.primary), StrokeKind::Outside);
    }

    let track_fill = if *on { pal.primary } else { pal.surface_container_high };
    let track_stroke = if *on {
        Stroke::NONE
    } else {
        Stroke::new(1.5, if resp.hovered() { pal.on_surface } else { pal.outline })
    };
    painter.rect(rect, radius, track_fill, track_stroke, StrokeKind::Inside);

    let thumb_r = egui::lerp(7.5..=10.0, how_on);
    let thumb_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
    let thumb_col = if *on { pal.on_primary } else { pal.outline };
    painter.circle_filled(Pos2::new(thumb_x, rect.center().y), thumb_r, thumb_col);
    resp
}

/// Material 3 Segmented Button Bar with drawn checkmark on the active segment.
fn md_segmented_profiles(
    ui: &mut egui::Ui,
    current_cap: Option<u32>,
    auto_enabled: bool,
    disabled: bool,
    pal: &Palette,
) -> Option<Option<u32>> {
    let mut clicked_cap: Option<Option<u32>> = None;
    let total_w = ui.available_width();
    let height = 38.0;

    Frame::new()
        .fill(if pal.is_light { pal.surface_container } else { Color32::TRANSPARENT })
        .stroke(Stroke::new(1.0, pal.outline_variant))
        .corner_radius(CornerRadius::same(19))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            let inner_w = (total_w - 6.0).max(180.0);
            let seg_w = inner_w / BUILTIN_PROFILES.len() as f32;
            let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(inner_w, height), Sense::hover());

            for (i, p) in BUILTIN_PROFILES.iter().enumerate() {
                let x0 = bar_rect.left() + i as f32 * seg_w;
                let seg_rect = Rect::from_min_size(Pos2::new(x0, bar_rect.top()), Vec2::new(seg_w, height));
                let id = ui.make_persistent_id(("profile_seg", p.name));
                let sense = if disabled { Sense::hover() } else { Sense::click() };
                let resp = ui.interact(seg_rect, id, sense);

                let selected = !auto_enabled && current_cap == p.cap_mhz;
                let painter = ui.painter();

                let fill = if selected {
                    pal.primary_container
                } else if resp.hovered() && !disabled {
                    pal.surface_container_high
                } else {
                    Color32::TRANSPARENT
                };

                let corners = CornerRadius {
                    nw: if i == 0 { 17 } else { 4 },
                    sw: if i == 0 { 17 } else { 4 },
                    ne: if i + 1 == BUILTIN_PROFILES.len() { 17 } else { 4 },
                    se: if i + 1 == BUILTIN_PROFILES.len() { 17 } else { 4 },
                };
                painter.rect_filled(seg_rect, corners, fill);

                if i > 0 && !selected {
                    painter.line_segment(
                        [Pos2::new(seg_rect.left(), seg_rect.top() + 8.0), Pos2::new(seg_rect.left(), seg_rect.bottom() - 8.0)],
                        Stroke::new(1.0, pal.outline_variant),
                    );
                }

                if resp.has_focus() {
                    painter.rect_stroke(seg_rect.shrink(1.0), corners, Stroke::new(1.5, pal.primary), StrokeKind::Inside);
                }

                let fg = if selected {
                    pal.on_primary_container
                } else if disabled {
                    pal.outline
                } else {
                    pal.on_surface
                };

                let label = match p.cap_mhz {
                    Some(m) => format!("{} {:.1} GHz", capitalize(p.name), m as f32 / 1000.0),
                    None => "Max (Full)".into(),
                };

                let text_offset = if selected { 7.0 } else { 0.0 };
                let text_center = Pos2::new(seg_rect.center().x + text_offset, seg_rect.center().y);
                let galley = painter.layout_no_wrap(label, FontId::proportional(12.5), fg);
                let text_pos = Pos2::new(text_center.x - galley.size().x * 0.5, text_center.y - galley.size().y * 0.5);
                if selected {
                    let check_center = Pos2::new(text_pos.x - 10.0, seg_rect.center().y);
                    paint_checkmark(painter, check_center, fg);
                }
                painter.galley(text_pos, galley, fg);

                if resp.clicked() && !disabled {
                    clicked_cap = Some(p.cap_mhz);
                }
            }
        });

    clicked_cap
}

fn draw_temp_graph(ui: &mut egui::Ui, history: &VecDeque<f32>, auto_cfg: Option<AutoConfig>, pal: &Palette) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 108.0), Sense::hover());
    let painter = ui.painter_at(rect);

    let min_t = 30.0f32;
    let max_t = 100.0f32;
    let pad_top = 12.0;
    let pad_bot = 8.0;
    let pad_left = 2.0;
    let pad_right = 34.0;
    let plot_w = (rect.width() - pad_left - pad_right).max(1.0);
    let y_for = |t: f32| -> f32 {
        let frac = ((t - min_t) / (max_t - min_t)).clamp(0.0, 1.0);
        rect.bottom() - pad_bot - frac * (rect.height() - pad_top - pad_bot)
    };

    for grid_t in [40.0f32, 60.0, 80.0, 100.0] {
        let y = y_for(grid_t);
        painter.line_segment(
            [Pos2::new(rect.left() + pad_left, y), Pos2::new(rect.right() - pad_right, y)],
            Stroke::new(1.0, pal.grid_line),
        );
        painter.text(
            Pos2::new(rect.right() - 2.0, y),
            Align2::RIGHT_CENTER,
            format!("{grid_t:.0}°"),
            FontId::monospace(10.0),
            pal.on_surface_variant,
        );
    }

    if let Some(cfg) = auto_cfg {
        for (t, label, base_col) in [(cfg.high_c, "HI", pal.error), (cfg.low_c, "LO", pal.cool)] {
            let col = Color32::from_rgba_unmultiplied(base_col.r(), base_col.g(), base_col.b(), 170);
            let y = y_for(t);
            painter.line_segment(
                [Pos2::new(rect.left() + pad_left, y), Pos2::new(rect.right() - pad_right, y)],
                Stroke::new(1.0, col),
            );
            painter.text(
                Pos2::new(rect.left() + pad_left + 2.0, y - 3.0),
                Align2::LEFT_BOTTOM,
                label,
                FontId::monospace(9.5),
                col,
            );
        }
    }

    if history.len() >= 2 {
        let step_x = plot_w / (HISTORY_LEN - 1) as f32;
        let start_x = rect.right() - pad_right - (history.len() - 1) as f32 * step_x;
        let pts: Vec<Pos2> = history
            .iter()
            .enumerate()
            .map(|(i, &t)| Pos2::new(start_x + i as f32 * step_x, y_for(t)))
            .collect();
        let line_color = history.back().copied().map(|c| pal.temp_color(c)).unwrap_or(pal.primary);

        let fill_alpha = if pal.is_light { 34 } else { 26 };
        let fill_color = Color32::from_rgba_unmultiplied(line_color.r(), line_color.g(), line_color.b(), fill_alpha);
        let base_y = rect.bottom() - pad_bot;
        for w in pts.windows(2) {
            let quad = vec![w[0], w[1], Pos2::new(w[1].x, base_y), Pos2::new(w[0].x, base_y)];
            painter.add(Shape::convex_polygon(quad, fill_color, Stroke::NONE));
        }

        painter.add(Shape::line(pts.clone(), Stroke::new(2.2, line_color)));

        if let Some(&last) = pts.last() {
            painter.circle_filled(last, 5.5, Color32::from_rgba_unmultiplied(line_color.r(), line_color.g(), line_color.b(), 55));
            painter.circle_filled(last, 3.0, line_color);
        }

        if let Some(hover_pos) = response.hover_pos() {
            if let Some((idx, &pt)) = pts
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| (a.x - hover_pos.x).abs().total_cmp(&(b.x - hover_pos.x).abs()))
            {
                if (pt.x - hover_pos.x).abs() < 16.0 {
                    let t = history[idx];
                    painter.line_segment(
                        [Pos2::new(pt.x, rect.top() + pad_top), Pos2::new(pt.x, base_y)],
                        Stroke::new(1.0, pal.outline_variant),
                    );
                    painter.circle_filled(pt, 4.0, pal.on_surface);
                    let pill_w = 48.0;
                    let pill_h = 20.0;
                    let px = (pt.x - pill_w * 0.5).clamp(rect.left() + 4.0, rect.right() - pad_right - pill_w - 4.0);
                    let py = (pt.y - pill_h - 8.0).max(rect.top() + 2.0);
                    let pill = Rect::from_min_size(Pos2::new(px, py), Vec2::new(pill_w, pill_h));
                    painter.rect_filled(pill, 6.0, pal.surface_container_high);
                    painter.rect_stroke(pill, 6.0, Stroke::new(1.0, pal.outline_variant), StrokeKind::Inside);
                    painter.text(
                        pill.center(),
                        Align2::CENTER_CENTER,
                        format!("{t:.1}°C"),
                        FontId::monospace(10.5),
                        pal.on_surface,
                    );
                }
            }
        }
    } else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Waiting for sensor readings...",
            FontId::proportional(12.5),
            pal.on_surface_variant,
        );
    }
}

fn section_divider(ui: &mut egui::Ui, pal: &Palette) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment(
        [Pos2::new(rect.left(), rect.center().y), Pos2::new(rect.right(), rect.center().y)],
        Stroke::new(1.0, pal.outline_variant),
    );
    ui.add_space(4.0);
}

impl eframe::App for CoolerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let pal = Palette::for_theme(self.settings.light_theme);
        let mut trigger_cap: Option<(Option<u32>, bool)> = None;
        let mut wake_worker = false;

        if self.relaunch_ready.load(Ordering::SeqCst) {
            self.quitting = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        let (tray_temp, tray_cap) = {
            let s = self.sync.state.lock().unwrap();
            (
                s.telemetry.as_ref().and_then(|t| t.temp_c),
                s.telemetry.as_ref().and_then(|t| t.cap_mhz),
            )
        };
        if let Some(tray) = &mut self.tray {
            tray.update_temp(tray_temp, tray_cap);
            for action in tray.poll() {
                match action {
                    tray::TrayAction::Show => {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    tray::TrayAction::SetCap(cap) => {
                        trigger_cap = Some((cap, true));
                        if let Some(m) = cap {
                            self.slider_mhz = m;
                        }
                        self.slider_dirty = false;
                    }
                    tray::TrayAction::Quit => {
                        self.quitting = true;
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            if !self.quitting && ui.ctx().input(|i| i.viewport().close_requested()) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }

        let (footer_text, footer_err) = {
            let s = self.sync.state.lock().unwrap();
            (s.status.clone(), s.is_error)
        };
        egui::Panel::bottom("status_bar")
            .frame(
                Frame::new()
                    .fill(pal.surface_container)
                    .stroke(Stroke::new(1.0, pal.outline_variant))
                    .inner_margin(Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| {
                let col = if footer_err { pal.error } else { pal.on_surface_variant };
                ui.label(RichText::new(footer_text).size(12.0).color(col));
            });

        egui::CentralPanel::default()
            .frame(Frame::new().fill(pal.surface).inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let mut s = self.sync.state.lock().unwrap();
                    let current_cap = s.telemetry.as_ref().and_then(|t| t.cap_mhz);
                    let applying = s.applying;

                    if !self.slider_synced {
                        if let Some(cap) = current_cap {
                            self.slider_mhz = cap;
                            self.slider_synced = true;
                        }
                    }

                    // Top App Bar
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(RichText::new("kool-cpu").size(20.0).strong());
                            let cpu = s
                                .telemetry
                                .as_ref()
                                .map(|t| t.cpu_name.as_str())
                                .unwrap_or("Detecting processor...");
                            ui.label(RichText::new(cpu).size(12.0).color(pal.on_surface_variant));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let has_update = matches!(
                                *self.update_state.lock().unwrap(),
                                updater::UpdateState::Available(_)
                            );
                            let (icon, tip, dot) = match self.page {
                                Page::Monitor => (
                                    MdIcon::Settings,
                                    if has_update { "Settings (update available)" } else { "Settings" },
                                    has_update,
                                ),
                                Page::Settings => (MdIcon::ArrowBack, "Back to monitor", false),
                            };
                            if md_icon_button(ui, icon, tip, dot, &pal).clicked() {
                                self.page = match self.page {
                                    Page::Monitor => Page::Settings,
                                    Page::Settings => Page::Monitor,
                                };
                            }

                            let (label, bg, fg) = if s.is_error {
                                ("Attention", pal.error_container, pal.error)
                            } else if applying {
                                ("Applying", pal.surface_container_high, pal.primary)
                            } else if s.auto_enabled {
                                ("Auto", pal.primary_container, pal.on_primary_container)
                            } else if current_cap.is_some() {
                                ("Capped", pal.primary_container, pal.on_primary_container)
                            } else {
                                ("Full speed", pal.surface_container_high, pal.on_surface_variant)
                            };
                            md_chip(ui, label, bg, fg);
                        });
                    });

                    ui.add_space(6.0);

                    if self.page == Page::Settings {
                        drop(s);
                        self.render_settings(ui, &pal);
                        return;
                    }

                    let card_stroke = if pal.is_light {
                        Stroke::new(1.0, pal.outline_variant)
                    } else {
                        Stroke::NONE
                    };

                    // Elevated Telemetry + Graph surface
                    Frame::new()
                        .fill(pal.surface_container)
                        .stroke(card_stroke)
                        .corner_radius(CornerRadius::same(16))
                        .inner_margin(Margin::same(16))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                match s.telemetry.as_ref().and_then(|t| t.temp_c) {
                                    Some(c) => {
                                        ui.label(
                                            RichText::new(format!("{c:.1}°C"))
                                                .font(FontId::monospace(32.0))
                                                .strong()
                                                .color(pal.temp_color(c)),
                                        );
                                    }
                                    None => {
                                        ui.label(
                                            RichText::new("--.-°C")
                                                .font(FontId::monospace(32.0))
                                                .strong()
                                                .color(pal.on_surface_variant),
                                        );
                                    }
                                }

                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        let cap_str = match current_cap {
                                            Some(m) => format!("{:.2} GHz cap", m as f32 / 1000.0),
                                            None => "Unlimited".into(),
                                        };
                                        ui.label(RichText::new(cap_str).font(FontId::monospace(14.0)).strong());
                                        if !s.history.is_empty() {
                                            let min = s.history.iter().copied().reduce(f32::min).unwrap_or(0.0);
                                            let max = s.history.iter().copied().reduce(f32::max).unwrap_or(0.0);
                                            let avg = s.history.iter().sum::<f32>() / s.history.len() as f32;
                                            ui.label(
                                                RichText::new(format!("{min:.0}° / {avg:.0}° / {max:.0}°"))
                                                    .font(FontId::monospace(11.5))
                                                    .color(pal.on_surface_variant),
                                            )
                                            .on_hover_text("2-minute Min / Avg / Max");
                                        }
                                    });
                                });
                            });

                            ui.add_space(6.0);
                            draw_temp_graph(ui, &s.history, s.auto_enabled.then_some(s.auto_cfg), &pal);
                        });

                    ui.add_space(8.0);

                    // Power profile section
                    ui.label(RichText::new("Power profile").size(13.5).strong());
                    if let Some(cap) = md_segmented_profiles(ui, current_cap, s.auto_enabled, applying, &pal) {
                        trigger_cap = Some((cap, true));
                        if let Some(m) = cap {
                            self.slider_mhz = m;
                        }
                        self.slider_dirty = false;
                    }

                    section_divider(ui, &pal);

                    // Manual frequency cap section
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Manual cap").size(13.5).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:.2} GHz ({} MHz)", self.slider_mhz as f32 / 1000.0, self.slider_mhz))
                                    .font(FontId::monospace(12.5))
                                    .color(if self.slider_dirty { pal.primary } else { pal.on_surface_variant }),
                            );
                        });
                    });

                    ui.add_enabled_ui(!s.auto_enabled && !applying, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().slider_width = (ui.available_width() - 94.0).max(140.0);
                            let slider_resp = ui.add(
                                egui::Slider::new(&mut self.slider_mhz, 1200..=5000)
                                    .step_by(100.0)
                                    .show_value(false),
                            );
                            if slider_resp.changed() {
                                self.slider_dirty = current_cap != Some(self.slider_mhz);
                            }

                            let (bg, fg) = if self.slider_dirty {
                                (pal.primary, pal.on_primary)
                            } else {
                                (pal.surface_container_high, pal.on_surface_variant)
                            };
                            let apply_btn = egui::Button::new(RichText::new("Apply").size(13.0).strong().color(fg))
                                .fill(bg)
                                .corner_radius(CornerRadius::same(18))
                                .min_size(Vec2::new(82.0, 34.0));
                            if ui.add_enabled(self.slider_dirty, apply_btn).clicked() {
                                trigger_cap = Some((Some(self.slider_mhz), false));
                                self.slider_dirty = false;
                            }
                        });
                    });

                    section_divider(ui, &pal);

                    // Auto thermal mode section
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(RichText::new("Auto thermal mode").size(13.5).strong());
                            ui.label(
                                RichText::new("Steps clock down when hot and restores speed when cool")
                                    .size(12.0)
                                    .color(pal.on_surface_variant),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if md_switch(ui, &mut s.auto_enabled, &pal).changed() {
                                wake_worker = true;
                            }
                        });
                    });

                    if s.auto_enabled {
                        ui.add_space(4.0);
                        Frame::new()
                            .fill(pal.surface_container)
                            .stroke(card_stroke)
                            .corner_radius(CornerRadius::same(12))
                            .inner_margin(Margin::same(12))
                            .show(ui, |ui| {
                                let mut cfg_changed = false;
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("Throttle ≥").size(12.0).color(pal.on_surface_variant));
                                    cfg_changed |= ui.add(egui::DragValue::new(&mut s.auto_cfg.high_c).range(50.0..=95.0).suffix(" °C")).changed();
                                    ui.add_space(12.0);
                                    ui.label(RichText::new("Relax ≤").size(12.0).color(pal.on_surface_variant));
                                    cfg_changed |= ui.add(egui::DragValue::new(&mut s.auto_cfg.low_c).range(40.0..=90.0).suffix(" °C")).changed();
                                });
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("Min").size(12.0).color(pal.on_surface_variant));
                                    cfg_changed |= ui.add(egui::DragValue::new(&mut s.auto_cfg.min_mhz).range(800..=4000).speed(100).suffix(" MHz")).changed();
                                    ui.label(RichText::new("Max").size(12.0).color(pal.on_surface_variant));
                                    cfg_changed |= ui.add(egui::DragValue::new(&mut s.auto_cfg.max_mhz).range(1200..=5500).speed(100).suffix(" MHz")).changed();
                                    ui.label(RichText::new("Step").size(12.0).color(pal.on_surface_variant));
                                    cfg_changed |= ui.add(egui::DragValue::new(&mut s.auto_cfg.step_mhz).range(100..=500).speed(50).suffix(" MHz")).changed();
                                });
                                if cfg_changed {
                                    wake_worker = true;
                                }
                                if let Err(msg) = s.auto_cfg.validate() {
                                    ui.label(RichText::new(msg).size(11.5).color(pal.error));
                                }
                            });
                    }
                });
            });

        if let Some((cap, disable_auto)) = trigger_cap {
            self.settings.last_cap_mhz = cap;
            if disable_auto {
                self.settings.last_auto_enabled = false;
            }
            let _ = self.settings.save();
            self.sync.request_cap(cap, disable_auto);
        } else if wake_worker {
            let (auto_on, auto_cfg) = {
                let s = self.sync.state.lock().unwrap();
                (s.auto_enabled, s.auto_cfg)
            };
            self.settings.last_auto_enabled = auto_on;
            self.settings.auto_cfg = auto_cfg;
            let _ = self.settings.save();
            self.sync.notify();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.settings.reset_on_exit && !self.relaunch_ready.load(Ordering::SeqCst) {
            let _ = native_backend().set_cap_mhz(None);
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn main() -> eframe::Result {
    updater::cleanup_old_binary();
    let launched_at_startup = std::env::args().any(|a| a == "--startup");
    let app_settings = settings::AppSettings::load();
    let start_visible = !(launched_at_startup && app_settings.show_in_tray);

    let app_icon = egui::IconData {
        rgba: icons::render_app_icon_rgba(64),
        width: 64,
        height: 64,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([450.0, 580.0])
            .with_min_inner_size([390.0, 460.0])
            .with_icon(app_icon)
            .with_visible(start_visible),
        ..Default::default()
    };
    eframe::run_native(
        "kool-cpu",
        options,
        Box::new(move |cc| Ok(Box::new(CoolerApp::new(cc, app_settings, launched_at_startup)))),
    )
}
