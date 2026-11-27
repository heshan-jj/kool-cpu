#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Cross-platform Material 3-styled egui GUI for cooler (Windows & Linux).

use cooler_backend::native_backend;
use cooler_core::{AutoAction, AutoConfig, AutoController, Backend, Telemetry, BUILTIN_PROFILES};
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, Pos2, RichText, Sense, Stroke, StrokeKind, Vec2};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

const HISTORY_LEN: usize = 60;
const POLL_SECS: u64 = 2;

// Material Design 3 Dark tonal palette (blue seed)
const MD_SURFACE: Color32 = Color32::from_rgb(17, 19, 24);
const MD_SURFACE_CONTAINER: Color32 = Color32::from_rgb(29, 32, 38);
const MD_SURFACE_CONTAINER_HIGH: Color32 = Color32::from_rgb(40, 43, 51);
const MD_OUTLINE: Color32 = Color32::from_rgb(142, 144, 153);
const MD_OUTLINE_VARIANT: Color32 = Color32::from_rgb(68, 71, 78);
const MD_ON_SURFACE: Color32 = Color32::from_rgb(227, 226, 230);
const MD_ON_SURFACE_VARIANT: Color32 = Color32::from_rgb(196, 198, 208);
const MD_PRIMARY: Color32 = Color32::from_rgb(168, 199, 250);
const MD_ON_PRIMARY: Color32 = Color32::from_rgb(6, 46, 111);
const MD_PRIMARY_CONTAINER: Color32 = Color32::from_rgb(0, 74, 119);
const MD_ON_PRIMARY_CONTAINER: Color32 = Color32::from_rgb(194, 231, 255);
const MD_ERROR: Color32 = Color32::from_rgb(242, 184, 181);
const MD_ERROR_CONTAINER: Color32 = Color32::from_rgb(140, 29, 24);
const MD_COOL: Color32 = Color32::from_rgb(125, 220, 160);
const MD_WARM: Color32 = Color32::from_rgb(246, 195, 96);

struct SharedState {
    telemetry: Option<Telemetry>,
    history: VecDeque<f32>,
    auto_enabled: bool,
    auto_cfg: AutoConfig,
    pending_cap: Option<Option<u32>>,
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
            status: "Starting...".into(),
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
        s.status = "Applying...".into();
        s.is_error = false;
        self.wake.notify_one();
    }

    fn notify(&self) {
        self.wake.notify_one();
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
                match res {
                    Ok(()) => {
                        s.status = match cap {
                            Some(m) => format!("Cap set to {m} MHz ({:.2} GHz)", m as f32 / 1000.0),
                            None => "Cap removed (unlimited)".into(),
                        };
                        s.is_error = false;
                    }
                    Err(e) => {
                        s.status = e;
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
                            if s.status == "Starting..." {
                                s.status = "Monitoring active".into();
                            }
                        } else if s.status == "Starting..." {
                            s.status = t.temp_source.clone();
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
                                    s.status = format!("Auto: lowered cap to {m} MHz");
                                    s.is_error = false;
                                }
                                Ok(AutoAction::Relaxed(c)) => {
                                    let label = c.map_or("unlimited".into(), |m| format!("{m} MHz"));
                                    let mut s = sync.state.lock().unwrap();
                                    s.status = format!("Auto: raised cap to {label}");
                                    s.is_error = false;
                                }
                                Ok(AutoAction::NoTemp) => {
                                    let mut s = sync.state.lock().unwrap();
                                    s.status = format!("Auto paused: {}", t.temp_source);
                                    s.is_error = true;
                                }
                                Ok(AutoAction::Hold) => {}
                                Err(e) => {
                                    let mut s = sync.state.lock().unwrap();
                                    s.status = format!("Auto error: {e}");
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
                    s.status = format!("Telemetry error: {e}");
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

struct CoolerApp {
    sync: Arc<WorkerSync>,
    slider_mhz: u32,
    slider_synced: bool,
}

impl CoolerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_material3(&cc.egui_ctx);
        let sync = WorkerSync::new();
        spawn_worker(Arc::clone(&sync), cc.egui_ctx.clone());
        Self {
            sync,
            slider_mhz: 3100,
            slider_synced: false,
        }
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
    visuals.widgets.inactive.corner_radius = CornerRadius::same(20);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(52, 56, 66);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, MD_ON_SURFACE);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(20);
    visuals.widgets.active.bg_fill = MD_PRIMARY;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, MD_ON_PRIMARY);
    visuals.widgets.active.corner_radius = CornerRadius::same(20);
    visuals.selection.bg_fill = MD_PRIMARY;
    visuals.selection.stroke = Stroke::new(1.0, MD_ON_PRIMARY);

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = visuals;
    style.spacing.item_spacing = Vec2::new(8.0, 10.0);
    style.spacing.button_padding = Vec2::new(16.0, 8.0);
    style.spacing.slider_rail_height = 6.0;
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.set_theme(egui::Theme::Dark);
}

fn md_card() -> Frame {
    Frame::new()
        .fill(MD_SURFACE_CONTAINER)
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(16))
}

fn md_chip(ui: &mut egui::Ui, text: &str, bg: Color32, fg: Color32) {
    Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin::symmetric(12, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).strong().color(fg));
        });
}

/// Material 3 pill toggle switch with smooth animated thumb.
fn md_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = Vec2::new(46.0, 26.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let how_on = ui.ctx().animate_bool_responsive(resp.id, *on);
    let painter = ui.painter();
    let radius = rect.height() * 0.5;
    let track_fill = if *on { MD_PRIMARY } else { MD_SURFACE_CONTAINER_HIGH };
    let track_stroke = if *on {
        Stroke::NONE
    } else {
        Stroke::new(1.5, MD_OUTLINE)
    };
    painter.rect(rect, radius, track_fill, track_stroke, StrokeKind::Inside);

    let thumb_r = egui::lerp(7.0..=9.5, how_on);
    let thumb_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
    let thumb_col = if *on { MD_ON_PRIMARY } else { MD_OUTLINE };
    painter.circle_filled(Pos2::new(thumb_x, rect.center().y), thumb_r, thumb_col);
    resp
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
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 132.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 12.0, MD_SURFACE);

    let min_t = 30.0f32;
    let max_t = 100.0f32;
    let pad_top = 12.0;
    let pad_bot = 12.0;
    let pad_left = 10.0;
    let pad_right = 36.0;
    let plot_w = (rect.width() - pad_left - pad_right).max(1.0);
    let y_for = |t: f32| -> f32 {
        let frac = ((t - min_t) / (max_t - min_t)).clamp(0.0, 1.0);
        rect.bottom() - pad_bot - frac * (rect.height() - pad_top - pad_bot)
    };

    for grid_t in [50.0f32, 70.0, 90.0] {
        let y = y_for(grid_t);
        painter.line_segment(
            [Pos2::new(rect.left() + pad_left, y), Pos2::new(rect.right() - pad_right, y)],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(227, 226, 230, 18)),
        );
        painter.text(
            Pos2::new(rect.right() - 6.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{grid_t:.0}°"),
            egui::FontId::proportional(10.5),
            MD_ON_SURFACE_VARIANT,
        );
    }

    if let Some(cfg) = auto_cfg {
        for (t, label, col) in [
            (cfg.high_c, "HI", Color32::from_rgba_unmultiplied(242, 184, 181, 140)),
            (cfg.low_c, "LO", Color32::from_rgba_unmultiplied(125, 220, 160, 140)),
        ] {
            let y = y_for(t);
            painter.line_segment(
                [Pos2::new(rect.left() + pad_left, y), Pos2::new(rect.right() - pad_right, y)],
                Stroke::new(1.0, col),
            );
            painter.text(
                Pos2::new(rect.left() + pad_left + 2.0, y - 5.0),
                egui::Align2::LEFT_BOTTOM,
                label,
                egui::FontId::proportional(9.5),
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

        let fill_color = Color32::from_rgba_unmultiplied(line_color.r(), line_color.g(), line_color.b(), 28);
        let base_y = rect.bottom() - pad_bot;
        for w in pts.windows(2) {
            let quad = vec![w[0], w[1], Pos2::new(w[1].x, base_y), Pos2::new(w[0].x, base_y)];
            painter.add(egui::Shape::convex_polygon(quad, fill_color, Stroke::NONE));
        }

        painter.add(egui::Shape::line(pts.clone(), Stroke::new(2.5, line_color)));

        if let Some(&last) = pts.last() {
            painter.circle_filled(last, 6.0, Color32::from_rgba_unmultiplied(line_color.r(), line_color.g(), line_color.b(), 55));
            painter.circle_filled(last, 3.2, line_color);
        }

        if let Some(hover_pos) = response.hover_pos() {
            if let Some((idx, &pt)) = pts
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| (a.x - hover_pos.x).abs().total_cmp(&(b.x - hover_pos.x).abs()))
            {
                if (pt.x - hover_pos.x).abs() < 16.0 {
                    let t = history[idx];
                    painter.circle_stroke(pt, 5.0, Stroke::new(1.5, MD_ON_SURFACE));
                    painter.text(
                        Pos2::new(pt.x, (pt.y - 12.0).max(rect.top() + 10.0)),
                        egui::Align2::CENTER_BOTTOM,
                        format!("{t:.1}°C"),
                        egui::FontId::proportional(11.0),
                        MD_ON_SURFACE,
                    );
                }
            }
        }
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Collecting temperature samples...",
            egui::FontId::proportional(12.5),
            MD_ON_SURFACE_VARIANT,
        );
    }
}

impl eframe::App for CoolerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut trigger_cap: Option<(Option<u32>, bool)> = None;
        let mut wake_worker = false;

        egui::CentralPanel::default()
            .frame(Frame::new().fill(MD_SURFACE).inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                let mut s = self.sync.state.lock().unwrap();
                let current_cap = s.telemetry.as_ref().and_then(|t| t.cap_mhz);

                if !self.slider_synced {
                    if let Some(cap) = current_cap {
                        self.slider_mhz = cap;
                        self.slider_synced = true;
                    }
                }

                // Material 3 Top App Bar
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Cooler").size(22.0).strong());
                        let cpu = s
                            .telemetry
                            .as_ref()
                            .map(|t| t.cpu_name.as_str())
                            .unwrap_or("Detecting CPU...");
                        ui.label(RichText::new(cpu).size(12.0).color(MD_ON_SURFACE_VARIANT));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (label, bg, fg) = if s.is_error {
                            ("Error", MD_ERROR_CONTAINER, MD_ERROR)
                        } else if s.auto_enabled {
                            ("Auto", MD_PRIMARY_CONTAINER, MD_ON_PRIMARY_CONTAINER)
                        } else if current_cap.is_some() {
                            ("Capped", MD_PRIMARY_CONTAINER, MD_ON_PRIMARY_CONTAINER)
                        } else {
                            ("Unlimited", MD_SURFACE_CONTAINER_HIGH, MD_ON_SURFACE_VARIANT)
                        };
                        md_chip(ui, label, bg, fg);
                    });
                });

                ui.add_space(4.0);

                // Telemetry Card
                md_card().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        match s.telemetry.as_ref().and_then(|t| t.temp_c) {
                            Some(c) => {
                                ui.label(RichText::new(format!("{c:.1}°")).size(40.0).strong().color(temp_color(c)));
                            }
                            None => {
                                ui.label(RichText::new("--°").size(40.0).strong().color(MD_ON_SURFACE_VARIANT));
                            }
                        }
                        ui.add_space(12.0);
                        ui.vertical(|ui| {
                            let cap_str = match current_cap {
                                Some(m) => format!("{:.2} GHz cap", m as f32 / 1000.0),
                                None => "No frequency cap".into(),
                            };
                            ui.label(RichText::new(cap_str).size(15.0).strong());
                            let src = s.telemetry.as_ref().map(|t| t.temp_source.as_str()).unwrap_or("");
                            ui.label(RichText::new(src).size(11.5).color(MD_ON_SURFACE_VARIANT));
                            if !s.history.is_empty() {
                                let min = s.history.iter().copied().reduce(f32::min).unwrap_or(0.0);
                                let max = s.history.iter().copied().reduce(f32::max).unwrap_or(0.0);
                                let avg = s.history.iter().sum::<f32>() / s.history.len() as f32;
                                ui.label(
                                    RichText::new(format!("Min {min:.0}°  •  Avg {avg:.0}°  •  Max {max:.0}°"))
                                        .size(11.5)
                                        .color(MD_ON_SURFACE_VARIANT),
                                );
                            }
                        });
                    });

                    ui.add_space(6.0);
                    draw_temp_graph(ui, &s.history, s.auto_enabled.then_some(s.auto_cfg));
                });

                // Profiles & Manual Cap Card
                md_card().show(ui, |ui| {
                    ui.label(RichText::new("POWER PROFILE").size(11.0).strong().color(MD_ON_SURFACE_VARIANT));

                    // Material 3 Segmented Button Bar (columns inside frame so all 3 fit cleanly)
                    Frame::new()
                        .stroke(Stroke::new(1.0, MD_OUTLINE_VARIANT))
                        .corner_radius(CornerRadius::same(20))
                        .inner_margin(Margin::same(2))
                        .show(ui, |ui| {
                            ui.columns(BUILTIN_PROFILES.len(), |cols| {
                                for (col, p) in cols.iter_mut().zip(BUILTIN_PROFILES.iter()) {
                                    let selected = !s.auto_enabled && current_cap == p.cap_mhz;
                                    let prefix = if selected { "✓ " } else { "" };
                                    let text = match p.cap_mhz {
                                        Some(m) => format!("{prefix}{} {:.1}G", capitalize(p.name), m as f32 / 1000.0),
                                        None => format!("{prefix}Max (Full)"),
                                    };
                                    let (bg, fg) = if selected {
                                        (MD_PRIMARY_CONTAINER, MD_ON_PRIMARY_CONTAINER)
                                    } else {
                                        (Color32::TRANSPARENT, MD_ON_SURFACE)
                                    };
                                    let btn = egui::Button::new(RichText::new(text).size(12.5).color(fg))
                                        .fill(bg)
                                        .stroke(Stroke::NONE)
                                        .corner_radius(CornerRadius::same(18))
                                        .min_size(Vec2::new(col.available_width(), 34.0));
                                    if col.add(btn).clicked() {
                                        trigger_cap = Some((p.cap_mhz, true));
                                        if let Some(m) = p.cap_mhz {
                                            self.slider_mhz = m;
                                        }
                                    }
                                }
                            });
                        });

                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("MANUAL CAP").size(11.0).strong().color(MD_ON_SURFACE_VARIANT));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:.2} GHz ({} MHz)", self.slider_mhz as f32 / 1000.0, self.slider_mhz))
                                    .size(12.0)
                                    .strong()
                                    .color(MD_PRIMARY),
                            );
                        });
                    });

                    ui.add_enabled_ui(!s.auto_enabled, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().slider_width = (ui.available_width() - 92.0).max(140.0);
                            ui.add(
                                egui::Slider::new(&mut self.slider_mhz, 1200..=4600)
                                    .step_by(100.0)
                                    .show_value(false),
                            );
                            let changed = current_cap != Some(self.slider_mhz);
                            let (bg, fg) = if changed {
                                (MD_PRIMARY, MD_ON_PRIMARY)
                            } else {
                                (MD_SURFACE_CONTAINER_HIGH, MD_ON_SURFACE_VARIANT)
                            };
                            let apply_btn = egui::Button::new(RichText::new("Apply").strong().color(fg))
                                .fill(bg)
                                .corner_radius(CornerRadius::same(20))
                                .min_size(Vec2::new(78.0, 32.0));
                            if ui.add(apply_btn).clicked() {
                                trigger_cap = Some((Some(self.slider_mhz), false));
                            }
                        });
                    });
                });

                // Auto Thermal Mode Card
                md_card().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Auto Thermal Mode").size(14.5).strong());
                            ui.label(
                                RichText::new("Automatically lowers clock when hot and relaxes when cool")
                                    .size(11.5)
                                    .color(MD_ON_SURFACE_VARIANT),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if md_switch(ui, &mut s.auto_enabled).changed() {
                                wake_worker = true;
                            }
                        });
                    });

                    if s.auto_enabled {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Throttle ≥").size(12.0).color(MD_ON_SURFACE_VARIANT));
                            ui.add(egui::DragValue::new(&mut s.auto_cfg.high_c).range(50.0..=95.0).suffix(" °C"));
                            ui.add_space(10.0);
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
                    }
                });

                // Footer status line
                let status_col = if s.is_error { MD_ERROR } else { MD_ON_SURFACE_VARIANT };
                ui.label(RichText::new(&s.status).size(11.5).color(status_col));
            });

        if let Some((cap, disable_auto)) = trigger_cap {
            self.sync.request_cap(cap, disable_auto);
        } else if wake_worker {
            self.sync.notify();
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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 560.0])
            .with_min_inner_size([420.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native("Cooler", options, Box::new(|cc| Ok(Box::new(CoolerApp::new(cc)))))
}
