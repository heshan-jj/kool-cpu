#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Cross-platform Material Design 3 egui GUI for kool-cpu (Windows & Linux).

mod settings;
mod tray;

use kool_cpu_backend::native_backend;
use kool_cpu_core::{AutoAction, AutoConfig, AutoController, Backend, Telemetry, BUILTIN_PROFILES};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, Pos2, Rect,
    RichText, ScrollArea, Sense, Shape, Stroke, StrokeKind, Vec2,
};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

const HISTORY_LEN: usize = 60;
const POLL_SECS: u64 = 2;

// High-contrast Material 3 Dark palette (all text pairs >= 8:1 WCAG AAA)
const MD_SURFACE: Color32 = Color32::from_rgb(16, 19, 24);
const MD_SURFACE_CONTAINER: Color32 = Color32::from_rgb(24, 28, 36);
const MD_SURFACE_CONTAINER_HIGH: Color32 = Color32::from_rgb(35, 41, 52);
const MD_OUTLINE: Color32 = Color32::from_rgb(122, 132, 150);
const MD_OUTLINE_VARIANT: Color32 = Color32::from_rgb(54, 61, 74);
const MD_ON_SURFACE: Color32 = Color32::from_rgb(241, 243, 247);
const MD_ON_SURFACE_VARIANT: Color32 = Color32::from_rgb(167, 176, 192);
const MD_PRIMARY: Color32 = Color32::from_rgb(122, 184, 255);
const MD_ON_PRIMARY: Color32 = Color32::from_rgb(11, 26, 46);
const MD_PRIMARY_CONTAINER: Color32 = Color32::from_rgb(22, 58, 95);
const MD_ON_PRIMARY_CONTAINER: Color32 = Color32::from_rgb(216, 235, 255);
const MD_ERROR: Color32 = Color32::from_rgb(248, 113, 113);
const MD_ERROR_CONTAINER: Color32 = Color32::from_rgb(92, 26, 30);
const MD_COOL: Color32 = Color32::from_rgb(110, 231, 183);
const MD_WARM: Color32 = Color32::from_rgb(251, 191, 36);

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
    page: Page,
    quitting: bool,
    slider_mhz: u32,
    slider_synced: bool,
    slider_dirty: bool,
}

impl CoolerApp {
    fn new(cc: &eframe::CreationContext<'_>, settings: settings::AppSettings, launched_at_startup: bool) -> Self {
        configure_material3(&cc.egui_ctx);
        let sync = WorkerSync::new();
        if launched_at_startup {
            // Startup mode always begins with cool mode / auto mode off (full default speed).
            sync.request_cap(None, true);
        }
        spawn_worker(Arc::clone(&sync), cc.egui_ctx.clone());
        let tray = if settings.show_in_tray {
            tray::Tray::new().ok()
        } else {
            None
        };
        Self {
            sync,
            tray,
            settings,
            page: Page::Monitor,
            quitting: false,
            slider_mhz: 3100,
            slider_synced: false,
            slider_dirty: false,
        }
    }

    fn set_status(&self, msg: impl Into<String>, is_error: bool) {
        let mut s = self.sync.state.lock().unwrap();
        s.status = msg.into();
        s.is_error = is_error;
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Settings").size(16.0).strong());
        ui.add_space(4.0);

        // Show in tray
        Frame::new()
            .fill(MD_SURFACE_CONTAINER)
            .corner_radius(CornerRadius::same(14))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    md_icon_badge(ui, MdIcon::Tray);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new("Show in system tray").size(13.5).strong());
                        ui.label(
                            RichText::new("Keep kool-cpu in the tray when you close the window")
                                .size(12.0)
                                .color(MD_ON_SURFACE_VARIANT),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if md_switch(ui, &mut self.settings.show_in_tray).changed() {
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

                section_divider(ui);

                // Run on startup with cool mode off
                ui.horizontal(|ui| {
                    md_icon_badge(ui, MdIcon::Power);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new("Run on startup (cool mode off)").size(13.5).strong());
                        ui.label(
                            RichText::new("Launch at sign-in with capping and Auto mode disabled")
                                .size(12.0)
                                .color(MD_ON_SURFACE_VARIANT),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if md_switch(ui, &mut self.settings.run_on_startup).changed() {
                            match settings::apply_startup(self.settings.run_on_startup) {
                                Ok(()) => {
                                    if let Err(e) = self.settings.save() {
                                        self.set_status(e, true);
                                    } else {
                                        let msg = if self.settings.run_on_startup {
                                            "Startup enabled (launches with cool mode off)"
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

                section_divider(ui);

                // Reset throttle on exit
                ui.horizontal(|ui| {
                    md_icon_badge(ui, MdIcon::Reset);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new("Reset CPU speed on exit").size(13.5).strong());
                        ui.label(
                            RichText::new("Restore full / default speed when kool-cpu closes or quits")
                                .size(12.0)
                                .color(MD_ON_SURFACE_VARIANT),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if md_switch(ui, &mut self.settings.reset_on_exit).changed() {
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
        ui.label(
            RichText::new(format!("Saved to {}", settings::AppSettings::config_path().display()))
                .size(11.0)
                .color(MD_ON_SURFACE_VARIANT),
        );
    }
}

fn configure_material3(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = MD_SURFACE;
    visuals.window_fill = MD_SURFACE;
    visuals.override_text_color = Some(MD_ON_SURFACE);
    visuals.widgets.noninteractive.bg_fill = MD_SURFACE_CONTAINER;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, MD_ON_SURFACE);
    visuals.widgets.inactive.bg_fill = MD_SURFACE_CONTAINER_HIGH;
    visuals.widgets.inactive.weak_bg_fill = MD_SURFACE_CONTAINER_HIGH;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, MD_ON_SURFACE);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(18);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(50, 55, 67);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, MD_ON_SURFACE);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, MD_OUTLINE_VARIANT);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(18);
    // NOTE: egui uses `widgets.active.fg_stroke.color` for `RichText::strong()`, so keep it bright!
    visuals.widgets.active.bg_fill = MD_PRIMARY_CONTAINER;
    visuals.widgets.active.fg_stroke = Stroke::new(1.5, MD_ON_SURFACE);
    visuals.widgets.active.corner_radius = CornerRadius::same(18);
    visuals.selection.bg_fill = MD_PRIMARY;
    visuals.selection.stroke = Stroke::new(1.5, MD_PRIMARY);
    visuals.slider_trailing_fill = true;

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = visuals;
    style.spacing.item_spacing = Vec2::new(8.0, 10.0);
    style.spacing.button_padding = Vec2::new(14.0, 8.0);
    style.spacing.slider_rail_height = 6.0;
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.set_theme(egui::Theme::Dark);
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
    Tray,
    Power,
    Reset,
}

fn paint_md_icon(painter: &egui::Painter, center: Pos2, icon: MdIcon, color: Color32) {
    let stroke = Stroke::new(1.8, color);
    match icon {
        MdIcon::Settings => {
            // Material gear: inner ring + outer ring + 8 radial teeth
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
            // Material arrow_back: horizontal shaft + 45-degree chevron
            let left = Pos2::new(center.x - 6.0, center.y);
            let right = Pos2::new(center.x + 6.0, center.y);
            painter.line_segment([left, right], stroke);
            painter.line_segment([left, Pos2::new(center.x - 1.0, center.y - 5.0)], stroke);
            painter.line_segment([left, Pos2::new(center.x - 1.0, center.y + 5.0)], stroke);
        }
        MdIcon::Tray => {
            // Material dock/tray: rounded monitor box with bottom bar
            let r = Rect::from_center_size(center, Vec2::new(14.0, 12.0));
            painter.rect_stroke(r, 2.5, stroke, StrokeKind::Inside);
            painter.line_segment(
                [Pos2::new(r.left() + 2.0, r.bottom() - 3.5), Pos2::new(r.right() - 2.0, r.bottom() - 3.5)],
                Stroke::new(1.5, color),
            );
        }
        MdIcon::Power => {
            // Material power_settings_new: open ring + top vertical stem
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
            // Material restart_alt: 270-degree circular arc + arrowhead
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
    }
}

/// Material 3 circular 36x36 IconButton with hover/focus states.
fn md_icon_button(ui: &mut egui::Ui, icon: MdIcon, tooltip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
    let painter = ui.painter();
    let radius = rect.width() * 0.5;
    let fill = if resp.is_pointer_button_down_on() {
        MD_PRIMARY_CONTAINER
    } else if resp.hovered() {
        MD_SURFACE_CONTAINER_HIGH
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        painter.circle_filled(rect.center(), radius, fill);
    }
    if resp.has_focus() {
        painter.circle_stroke(rect.center(), radius, Stroke::new(1.5, MD_PRIMARY));
    }
    let fg = if resp.hovered() { MD_ON_SURFACE } else { MD_ON_SURFACE_VARIANT };
    paint_md_icon(painter, rect.center(), icon, fg);
    resp.on_hover_text(tooltip)
}

fn md_icon_badge(ui: &mut egui::Ui, icon: MdIcon) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), 16.0, MD_SURFACE_CONTAINER_HIGH);
    paint_md_icon(painter, rect.center(), icon, MD_PRIMARY);
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
fn md_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
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
        painter.rect_stroke(rect.expand(2.5), radius + 2.5, Stroke::new(1.5, MD_PRIMARY), StrokeKind::Outside);
    }

    let track_fill = if *on { MD_PRIMARY } else { MD_SURFACE_CONTAINER_HIGH };
    let track_stroke = if *on {
        Stroke::NONE
    } else {
        Stroke::new(1.5, if resp.hovered() { MD_ON_SURFACE } else { MD_OUTLINE })
    };
    painter.rect(rect, radius, track_fill, track_stroke, StrokeKind::Inside);

    let thumb_r = egui::lerp(7.5..=10.0, how_on);
    let thumb_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
    let thumb_col = if *on { MD_ON_PRIMARY } else { MD_OUTLINE };
    painter.circle_filled(Pos2::new(thumb_x, rect.center().y), thumb_r, thumb_col);
    resp
}

/// Material 3 Segmented Button Bar with drawn checkmark on the active segment.
fn md_segmented_profiles(
    ui: &mut egui::Ui,
    current_cap: Option<u32>,
    auto_enabled: bool,
    disabled: bool,
) -> Option<Option<u32>> {
    let mut clicked_cap: Option<Option<u32>> = None;
    let total_w = ui.available_width();
    let height = 38.0;

    Frame::new()
        .stroke(Stroke::new(1.0, MD_OUTLINE_VARIANT))
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
                    MD_PRIMARY_CONTAINER
                } else if resp.hovered() && !disabled {
                    Color32::from_rgba_unmultiplied(227, 226, 230, 14)
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

                // Subtle vertical divider between unselected adjacent segments
                if i > 0 && !selected {
                    painter.line_segment(
                        [Pos2::new(seg_rect.left(), seg_rect.top() + 8.0), Pos2::new(seg_rect.left(), seg_rect.bottom() - 8.0)],
                        Stroke::new(1.0, MD_OUTLINE_VARIANT),
                    );
                }

                if resp.has_focus() {
                    painter.rect_stroke(seg_rect.shrink(1.0), corners, Stroke::new(1.5, MD_PRIMARY), StrokeKind::Inside);
                }

                let fg = if selected {
                    MD_ON_PRIMARY_CONTAINER
                } else if disabled {
                    MD_OUTLINE
                } else {
                    MD_ON_SURFACE
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

fn temp_color(c: f32) -> Color32 {
    if c >= 80.0 {
        MD_ERROR
    } else if c >= 70.0 {
        MD_WARM
    } else {
        MD_COOL
    }
}

fn draw_temp_graph(ui: &mut egui::Ui, history: &VecDeque<f32>, auto_cfg: Option<AutoConfig>) {
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
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(227, 226, 230, 16)),
        );
        painter.text(
            Pos2::new(rect.right() - 2.0, y),
            Align2::RIGHT_CENTER,
            format!("{grid_t:.0}°"),
            FontId::monospace(10.0),
            MD_ON_SURFACE_VARIANT,
        );
    }

    if let Some(cfg) = auto_cfg {
        for (t, label, col) in [
            (cfg.high_c, "HI", Color32::from_rgba_unmultiplied(242, 184, 181, 150)),
            (cfg.low_c, "LO", Color32::from_rgba_unmultiplied(125, 220, 160, 150)),
        ] {
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
        let line_color = history.back().copied().map(temp_color).unwrap_or(MD_PRIMARY);

        let fill_color = Color32::from_rgba_unmultiplied(line_color.r(), line_color.g(), line_color.b(), 26);
        let base_y = rect.bottom() - pad_bot;
        for w in pts.windows(2) {
            let quad = vec![w[0], w[1], Pos2::new(w[1].x, base_y), Pos2::new(w[0].x, base_y)];
            painter.add(Shape::convex_polygon(quad, fill_color, Stroke::NONE));
        }

        painter.add(Shape::line(pts.clone(), Stroke::new(2.2, line_color)));

        if let Some(&last) = pts.last() {
            painter.circle_filled(last, 5.5, Color32::from_rgba_unmultiplied(line_color.r(), line_color.g(), line_color.b(), 50));
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
                        Stroke::new(1.0, Color32::from_rgba_unmultiplied(227, 226, 230, 45)),
                    );
                    painter.circle_filled(pt, 4.0, MD_ON_SURFACE);
                    let pill_w = 48.0;
                    let pill_h = 20.0;
                    let px = (pt.x - pill_w * 0.5).clamp(rect.left() + 4.0, rect.right() - pad_right - pill_w - 4.0);
                    let py = (pt.y - pill_h - 8.0).max(rect.top() + 2.0);
                    let pill = Rect::from_min_size(Pos2::new(px, py), Vec2::new(pill_w, pill_h));
                    painter.rect_filled(pill, 6.0, MD_SURFACE_CONTAINER_HIGH);
                    painter.rect_stroke(pill, 6.0, Stroke::new(1.0, MD_OUTLINE_VARIANT), StrokeKind::Inside);
                    painter.text(
                        pill.center(),
                        Align2::CENTER_CENTER,
                        format!("{t:.1}°C"),
                        FontId::monospace(10.5),
                        MD_ON_SURFACE,
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
            MD_ON_SURFACE_VARIANT,
        );
    }
}

fn section_divider(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment(
        [Pos2::new(rect.left(), rect.center().y), Pos2::new(rect.right(), rect.center().y)],
        Stroke::new(1.0, MD_OUTLINE_VARIANT),
    );
    ui.add_space(4.0);
}

impl eframe::App for CoolerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut trigger_cap: Option<(Option<u32>, bool)> = None;
        let mut wake_worker = false;

        // Handle system tray menu, left-click, and live temperature icon updates.
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
            // Hide to tray when user clicks X instead of exiting.
            if !self.quitting && ui.ctx().input(|i| i.viewport().close_requested()) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
            // Poll tray events responsively even while idle.
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }

        // Pinned footer status bar (never clips off-screen)
        let (footer_text, footer_err) = {
            let s = self.sync.state.lock().unwrap();
            (s.status.clone(), s.is_error)
        };
        egui::Panel::bottom("status_bar")
            .frame(
                Frame::new()
                    .fill(MD_SURFACE_CONTAINER)
                    .stroke(Stroke::new(1.0, MD_OUTLINE_VARIANT))
                    .inner_margin(Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| {
                let col = if footer_err { MD_ERROR } else { MD_ON_SURFACE_VARIANT };
                ui.label(RichText::new(footer_text).size(12.0).color(col));
            });

        egui::CentralPanel::default()
            .frame(Frame::new().fill(MD_SURFACE).inner_margin(Margin::same(16)))
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
                            ui.label(RichText::new(cpu).size(12.0).color(MD_ON_SURFACE_VARIANT));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let (icon, tip) = match self.page {
                                Page::Monitor => (MdIcon::Settings, "Settings"),
                                Page::Settings => (MdIcon::ArrowBack, "Back to monitor"),
                            };
                            if md_icon_button(ui, icon, tip).clicked() {
                                self.page = match self.page {
                                    Page::Monitor => Page::Settings,
                                    Page::Settings => Page::Monitor,
                                };
                            }

                            let (label, bg, fg) = if s.is_error {
                                ("Attention", MD_ERROR_CONTAINER, MD_ERROR)
                            } else if applying {
                                ("Applying", MD_SURFACE_CONTAINER_HIGH, MD_PRIMARY)
                            } else if s.auto_enabled {
                                ("Auto", MD_PRIMARY_CONTAINER, MD_ON_PRIMARY_CONTAINER)
                            } else if current_cap.is_some() {
                                ("Capped", MD_PRIMARY_CONTAINER, MD_ON_PRIMARY_CONTAINER)
                            } else {
                                ("Full speed", MD_SURFACE_CONTAINER_HIGH, MD_ON_SURFACE_VARIANT)
                            };
                            md_chip(ui, label, bg, fg);
                        });
                    });

                    ui.add_space(6.0);

                    if self.page == Page::Settings {
                        drop(s);
                        self.render_settings(ui);
                        return;
                    }

                    // Single elevated Telemetry + Graph surface
                    Frame::new()
                        .fill(MD_SURFACE_CONTAINER)
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
                                                .color(temp_color(c)),
                                        );
                                    }
                                    None => {
                                        ui.label(
                                            RichText::new("--.-°C")
                                                .font(FontId::monospace(32.0))
                                                .strong()
                                                .color(MD_ON_SURFACE_VARIANT),
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
                                                    .color(MD_ON_SURFACE_VARIANT),
                                            )
                                            .on_hover_text("2-minute Min / Avg / Max");
                                        }
                                    });
                                });
                            });

                            ui.add_space(6.0);
                            draw_temp_graph(ui, &s.history, s.auto_enabled.then_some(s.auto_cfg));
                        });

                    ui.add_space(8.0);

                    // Power profile section (directly on surface — no card spam)
                    ui.label(RichText::new("Power profile").size(13.5).strong());
                    if let Some(cap) = md_segmented_profiles(ui, current_cap, s.auto_enabled, applying) {
                        trigger_cap = Some((cap, true));
                        if let Some(m) = cap {
                            self.slider_mhz = m;
                        }
                        self.slider_dirty = false;
                    }

                    section_divider(ui);

                    // Manual frequency cap section
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Manual cap").size(13.5).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:.2} GHz ({} MHz)", self.slider_mhz as f32 / 1000.0, self.slider_mhz))
                                    .font(FontId::monospace(12.5))
                                    .color(if self.slider_dirty { MD_PRIMARY } else { MD_ON_SURFACE_VARIANT }),
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
                                (MD_PRIMARY, MD_ON_PRIMARY)
                            } else {
                                (MD_SURFACE_CONTAINER_HIGH, MD_ON_SURFACE_VARIANT)
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

                    section_divider(ui);

                    // Auto thermal mode section
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(RichText::new("Auto thermal mode").size(13.5).strong());
                            ui.label(
                                RichText::new("Steps clock down when hot and restores speed when cool")
                                    .size(12.0)
                                    .color(MD_ON_SURFACE_VARIANT),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if md_switch(ui, &mut s.auto_enabled).changed() {
                                wake_worker = true;
                            }
                        });
                    });

                    if s.auto_enabled {
                        ui.add_space(4.0);
                        Frame::new()
                            .fill(MD_SURFACE_CONTAINER)
                            .corner_radius(CornerRadius::same(12))
                            .inner_margin(Margin::same(12))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("Throttle ≥").size(12.0).color(MD_ON_SURFACE_VARIANT));
                                    ui.add(egui::DragValue::new(&mut s.auto_cfg.high_c).range(50.0..=95.0).suffix(" °C"));
                                    ui.add_space(12.0);
                                    ui.label(RichText::new("Relax ≤").size(12.0).color(MD_ON_SURFACE_VARIANT));
                                    ui.add(egui::DragValue::new(&mut s.auto_cfg.low_c).range(40.0..=90.0).suffix(" °C"));
                                });
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("Min").size(12.0).color(MD_ON_SURFACE_VARIANT));
                                    ui.add(egui::DragValue::new(&mut s.auto_cfg.min_mhz).range(800..=4000).speed(100).suffix(" MHz"));
                                    ui.label(RichText::new("Max").size(12.0).color(MD_ON_SURFACE_VARIANT));
                                    ui.add(egui::DragValue::new(&mut s.auto_cfg.max_mhz).range(1200..=5500).speed(100).suffix(" MHz"));
                                    ui.label(RichText::new("Step").size(12.0).color(MD_ON_SURFACE_VARIANT));
                                    ui.add(egui::DragValue::new(&mut s.auto_cfg.step_mhz).range(100..=500).speed(50).suffix(" MHz"));
                                });
                                if let Err(msg) = s.auto_cfg.validate() {
                                    ui.label(RichText::new(msg).size(11.5).color(MD_ERROR));
                                }
                            });
                    }
                });
            });

        if let Some((cap, disable_auto)) = trigger_cap {
            self.sync.request_cap(cap, disable_auto);
        } else if wake_worker {
            self.sync.notify();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.settings.reset_on_exit {
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
    let launched_at_startup = std::env::args().any(|a| a == "--startup");
    let app_settings = settings::AppSettings::load();
    let start_visible = !(launched_at_startup && app_settings.show_in_tray);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([450.0, 580.0])
            .with_min_inner_size([390.0, 460.0])
            .with_visible(start_visible),
        ..Default::default()
    };
    eframe::run_native(
        "kool-cpu",
        options,
        Box::new(move |cc| Ok(Box::new(CoolerApp::new(cc, app_settings, launched_at_startup)))),
    )
}
