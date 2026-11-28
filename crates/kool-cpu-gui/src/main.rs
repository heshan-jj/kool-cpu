#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Cross-platform Tauri v2 desktop app for kool-cpu (Windows & Linux).

mod icons;
mod settings;
mod updater;

use kool_cpu_backend::native_backend;
use kool_cpu_core::{AutoAction, AutoConfig, AutoController, Backend, PowerSource, Telemetry};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

const HISTORY_LEN: usize = 60;
const POLL_SECS: u64 = 2;
const TRAY_ID: &str = "kool-cpu-tray";
const CLICK_DEBOUNCE: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, Serialize)]
struct AutoCfgDto {
    high_c: f32,
    low_c: f32,
    min_mhz: u32,
    max_mhz: u32,
    step_mhz: u32,
}

impl From<AutoConfig> for AutoCfgDto {
    fn from(c: AutoConfig) -> Self {
        Self {
            high_c: c.high_c,
            low_c: c.low_c,
            min_mhz: c.min_mhz,
            max_mhz: c.max_mhz,
            step_mhz: c.step_mhz,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct SettingsDto {
    show_in_tray: bool,
    run_on_startup: bool,
    reset_on_exit: bool,
    light_theme: bool,
    ac_dc_enabled: bool,
    ac_cap_mhz: Option<u32>,
    dc_cap_mhz: Option<u32>,
}

impl From<&settings::AppSettings> for SettingsDto {
    fn from(s: &settings::AppSettings) -> Self {
        Self {
            show_in_tray: s.show_in_tray,
            run_on_startup: s.run_on_startup,
            reset_on_exit: s.reset_on_exit,
            light_theme: s.light_theme,
            ac_dc_enabled: s.ac_dc_enabled,
            ac_cap_mhz: s.ac_cap_mhz,
            dc_cap_mhz: s.dc_cap_mhz,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct UpdateDto {
    kind: &'static str,
    tag: Option<String>,
    message: Option<String>,
}

impl From<&updater::UpdateState> for UpdateDto {
    fn from(u: &updater::UpdateState) -> Self {
        match u {
            updater::UpdateState::Idle => Self { kind: "idle", tag: None, message: None },
            updater::UpdateState::Checking => Self { kind: "checking", tag: None, message: None },
            updater::UpdateState::UpToDate => Self { kind: "up_to_date", tag: None, message: None },
            updater::UpdateState::Available(info) => Self {
                kind: "available",
                tag: Some(info.tag.clone()),
                message: None,
            },
            updater::UpdateState::Updating => Self { kind: "updating", tag: None, message: None },
            updater::UpdateState::Error(e) => Self {
                kind: "error",
                tag: None,
                message: Some(e.clone()),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct UiSnapshot {
    cpu_name: String,
    temp_c: Option<f32>,
    cap_mhz: Option<u32>,
    power_source: &'static str,
    history: Vec<f32>,
    auto_enabled: bool,
    auto_cfg: AutoCfgDto,
    applying: bool,
    status: String,
    is_error: bool,
    settings: SettingsDto,
    update: UpdateDto,
    version: &'static str,
    config_path: String,
}

struct SharedState {
    telemetry: Option<Telemetry>,
    history: VecDeque<f32>,
    auto_enabled: bool,
    auto_cfg: AutoConfig,
    settings: settings::AppSettings,
    update: updater::UpdateState,
    ac_dc_reapply: bool,
    pending_cap: Option<Option<u32>>,
    applying: bool,
    status: String,
    is_error: bool,
}

impl SharedState {
    fn new(settings: settings::AppSettings, launched_at_startup: bool) -> Self {
        let auto_enabled = !launched_at_startup && settings.last_auto_enabled;
        let auto_cfg = settings.auto_cfg;
        let ac_dc_reapply = settings.ac_dc_enabled;
        Self {
            telemetry: None,
            history: VecDeque::with_capacity(HISTORY_LEN),
            auto_enabled,
            auto_cfg,
            settings,
            update: updater::UpdateState::Idle,
            ac_dc_reapply,
            pending_cap: None,
            applying: false,
            status: "Connecting to sensors...".into(),
            is_error: false,
        }
    }

    fn snapshot(&self) -> UiSnapshot {
        let (cpu_name, temp_c, cap_mhz, power_source) = match &self.telemetry {
            Some(t) => (
                t.cpu_name.clone(),
                t.temp_c,
                t.cap_mhz,
                match t.power_source {
                    PowerSource::Ac => "Ac",
                    PowerSource::Battery => "Battery",
                    PowerSource::Unknown => "Unknown",
                },
            ),
            None => (String::new(), None, None, "Unknown"),
        };
        UiSnapshot {
            cpu_name,
            temp_c,
            cap_mhz,
            power_source,
            history: self.history.iter().copied().collect(),
            auto_enabled: self.auto_enabled,
            auto_cfg: self.auto_cfg.into(),
            applying: self.applying,
            status: self.status.clone(),
            is_error: self.is_error,
            settings: SettingsDto::from(&self.settings),
            update: UpdateDto::from(&self.update),
            version: updater::CURRENT_VERSION,
            config_path: settings::AppSettings::config_path().display().to_string(),
        }
    }
}

struct AppSync {
    state: Mutex<SharedState>,
    wake: Condvar,
    quitting: AtomicBool,
    relaunch_ready: AtomicBool,
    last_tray_click: Mutex<Option<Instant>>,
}

impl AppSync {
    fn new(settings: settings::AppSettings, launched_at_startup: bool) -> Arc<Self> {
        let s = SharedState::new(settings, launched_at_startup);
        Arc::new(Self {
            state: Mutex::new(s),
            wake: Condvar::new(),
            quitting: AtomicBool::new(false),
            relaunch_ready: AtomicBool::new(false),
            last_tray_click: Mutex::new(None),
        })
    }

    fn emit(&self, app: &AppHandle) {
        let snap = self.state.lock().unwrap().snapshot();
        let _ = app.emit("state-changed", snap);
    }

    fn request_cap(&self, app: &AppHandle, cap: Option<u32>, disable_auto: bool, save: bool) {
        {
            let mut s = self.state.lock().unwrap();
            if disable_auto {
                s.auto_enabled = false;
                s.settings.last_auto_enabled = false;
            }
            if save {
                s.settings.last_cap_mhz = cap;
                let _ = s.settings.save();
            }
            s.pending_cap = Some(cap);
            s.applying = true;
            s.status = match cap {
                Some(m) => format!("Applying {:.2} GHz cap...", m as f32 / 1000.0),
                None => "Removing frequency cap...".into(),
            };
            s.is_error = false;
        }
        self.wake.notify_one();
        self.emit(app);
    }
}

fn friendly_error(raw: &str) -> String {
    if raw.contains("0x80070005") || raw.contains("Access denied") || raw.contains("Administrator") {
        "Administrator privileges required — right-click kool-cpu-gui.exe and Run as administrator".into()
    } else if raw.contains("PawnIO not installed") {
        "PawnIO driver not found — install from https://pawnio.eu/ for live CPU temps".into()
    } else {
        raw.to_string()
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn toggle_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let visible = w.is_visible().unwrap_or(false);
        let minimized = w.is_minimized().unwrap_or(false);
        if !visible || minimized {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        } else {
            let _ = w.hide();
        }
    }
}

fn update_tray_visuals(
    app: &AppHandle,
    temp_c: Option<f32>,
    cap_mhz: Option<u32>,
    last_temp: &mut Option<Option<i32>>,
    last_cap: &mut Option<Option<u32>>,
) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let rounded = temp_c.map(|c| c.round() as i32);
    let temp_changed = *last_temp != Some(rounded);
    if temp_changed {
        let img = Image::new_owned(icons::render_temp_tray_rgba(rounded), 32, 32);
        let _ = tray.set_icon(Some(img));
        *last_temp = Some(rounded);
    }
    if temp_changed || *last_cap != Some(cap_mhz) {
        let temp_str = temp_c.map_or("--°C".into(), |c| format!("{c:.1}°C"));
        let cap_str = cap_mhz.map_or("Full speed".into(), |m| format!("{:.2} GHz", m as f32 / 1000.0));
        let _ = tray.set_tooltip(Some(format!("kool-cpu — {temp_str} | {cap_str}")));
        *last_cap = Some(cap_mhz);
    }
}

fn spawn_worker(sync: Arc<AppSync>, app: AppHandle) {
    thread::spawn(move || {
        let backend: Box<dyn Backend> = native_backend();
        let mut controller: Option<AutoController> = None;
        let mut last_cfg = AutoConfig::default();
        let mut last_power: Option<PowerSource> = None;
        let mut last_tray_temp: Option<Option<i32>> = None;
        let mut last_tray_cap: Option<Option<u32>> = None;

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
                Ok(mut t) => {
                    let (auto_on, cfg, ac_dc_on, ac_cap, dc_cap, reapply) = {
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
                        let r = s.ac_dc_reapply;
                        s.ac_dc_reapply = false;
                        (
                            s.auto_enabled,
                            s.auto_cfg,
                            s.settings.ac_dc_enabled,
                            s.settings.ac_cap_mhz,
                            s.settings.dc_cap_mhz,
                            r,
                        )
                    };

                    // Auto-switch on AC <-> Battery transitions (or when just enabled).
                    if ac_dc_on && t.power_source != PowerSource::Unknown {
                        if last_power != Some(t.power_source) || reapply {
                            let target = match t.power_source {
                                PowerSource::Ac => ac_cap,
                                PowerSource::Battery => dc_cap,
                                PowerSource::Unknown => t.cap_mhz,
                            };
                            let pwr_label = match t.power_source {
                                PowerSource::Ac => "Plugged in",
                                PowerSource::Battery => "On battery",
                                PowerSource::Unknown => "Unknown",
                            };
                            match backend.set_cap_mhz(target) {
                                Ok(()) => {
                                    t.cap_mhz = target;
                                    let cap_label = target.map_or("Full speed".into(), |m| {
                                        format!("{:.2} GHz", m as f32 / 1000.0)
                                    });
                                    let mut s = sync.state.lock().unwrap();
                                    s.telemetry = Some(t.clone());
                                    s.auto_enabled = false;
                                    s.status = format!("{pwr_label} — switched to {cap_label}");
                                    s.is_error = false;
                                    controller = None;
                                }
                                Err(e) => {
                                    let mut s = sync.state.lock().unwrap();
                                    s.status = friendly_error(&e);
                                    s.is_error = true;
                                }
                            }
                        }
                    }
                    last_power = Some(t.power_source);

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
                                    t.cap_mhz = Some(m);
                                    let mut s = sync.state.lock().unwrap();
                                    s.telemetry = Some(t.clone());
                                    s.status = format!("Auto mode lowered cap to {:.2} GHz", m as f32 / 1000.0);
                                    s.is_error = false;
                                }
                                Ok(AutoAction::Relaxed(c)) => {
                                    t.cap_mhz = c;
                                    let label = c.map_or("full speed".into(), |m| format!("{:.2} GHz", m as f32 / 1000.0));
                                    let mut s = sync.state.lock().unwrap();
                                    s.telemetry = Some(t.clone());
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

                    update_tray_visuals(&app, t.temp_c, t.cap_mhz, &mut last_tray_temp, &mut last_tray_cap);
                }
                Err(e) => {
                    let mut s = sync.state.lock().unwrap();
                    s.status = friendly_error(&e);
                    s.is_error = true;
                }
            }

            sync.emit(&app);

            let guard = sync.state.lock().unwrap();
            if guard.pending_cap.is_none() {
                let _ = sync.wake.wait_timeout(guard, Duration::from_secs(POLL_SECS)).unwrap();
            }
        }
    });
}

fn spawn_update_check(sync: Arc<AppSync>, app: AppHandle, silent: bool) {
    sync.state.lock().unwrap().update = updater::UpdateState::Checking;
    sync.emit(&app);
    thread::spawn(move || {
        let next = match updater::check_latest() {
            Ok(Some(info)) => updater::UpdateState::Available(info),
            Ok(None) => updater::UpdateState::UpToDate,
            Err(_) if silent => updater::UpdateState::Idle,
            Err(e) => updater::UpdateState::Error(e),
        };
        sync.state.lock().unwrap().update = next;
        sync.emit(&app);
    });
}

#[tauri::command]
fn get_state(sync: State<'_, Arc<AppSync>>) -> UiSnapshot {
    sync.state.lock().unwrap().snapshot()
}

#[tauri::command]
fn set_cap(mhz: Option<u32>, disable_auto: bool, app: AppHandle, sync: State<'_, Arc<AppSync>>) {
    sync.request_cap(&app, mhz, disable_auto, true);
}

#[tauri::command]
fn set_auto(
    enabled: bool,
    high_c: f32,
    low_c: f32,
    min_mhz: u32,
    max_mhz: u32,
    step_mhz: u32,
    app: AppHandle,
    sync: State<'_, Arc<AppSync>>,
) {
    let cfg = AutoConfig {
        high_c,
        low_c,
        min_mhz,
        max_mhz,
        step_mhz,
    };
    {
        let mut s = sync.state.lock().unwrap();
        s.auto_enabled = enabled;
        s.auto_cfg = cfg;
        s.settings.last_auto_enabled = enabled;
        s.settings.auto_cfg = cfg;
        let _ = s.settings.save();
    }
    sync.wake.notify_one();
    sync.emit(&app);
}

#[tauri::command]
fn update_setting(key: String, value: bool, app: AppHandle, sync: State<'_, Arc<AppSync>>) {
    let mut wake = false;
    {
        let mut s = sync.state.lock().unwrap();
        match key.as_str() {
            "light_theme" => {
                s.settings.light_theme = value;
                s.status = if value { "Light theme enabled" } else { "Dark theme enabled" }.into();
                s.is_error = false;
            }
            "show_in_tray" => {
                s.settings.show_in_tray = value;
                if let Some(tray) = app.tray_by_id(TRAY_ID) {
                    let _ = tray.set_visible(value);
                }
                s.status = if value {
                    "System tray enabled"
                } else {
                    "System tray disabled — closing window will exit"
                }
                .into();
                s.is_error = false;
            }
            "run_on_startup" => match settings::apply_startup(value) {
                Ok(()) => {
                    s.settings.run_on_startup = value;
                    s.status = if value {
                        "Startup enabled (uses your saved speed)"
                    } else {
                        "Startup disabled"
                    }
                    .into();
                    s.is_error = false;
                }
                Err(e) => {
                    s.status = friendly_error(&e);
                    s.is_error = true;
                }
            },
            "ac_dc_enabled" => {
                s.settings.ac_dc_enabled = value;
                s.ac_dc_reapply = value;
                s.status = if value {
                    "Plugged-in / Battery switching enabled"
                } else {
                    "Plugged-in / Battery switching disabled"
                }
                .into();
                s.is_error = false;
                wake = true;
            }
            "reset_on_exit" => {
                s.settings.reset_on_exit = value;
                s.status = if value {
                    "CPU throttle will reset to full speed on exit"
                } else {
                    "CPU cap will persist after exit"
                }
                .into();
                s.is_error = false;
            }
            _ => {}
        }
        if let Err(e) = s.settings.save() {
            s.status = e;
            s.is_error = true;
        }
    }
    if wake {
        sync.wake.notify_one();
    }
    sync.emit(&app);
}

#[tauri::command]
fn set_power_profile(target: String, mhz: Option<u32>, app: AppHandle, sync: State<'_, Arc<AppSync>>) {
    {
        let mut s = sync.state.lock().unwrap();
        match target.as_str() {
            "ac" => s.settings.ac_cap_mhz = mhz,
            "dc" => s.settings.dc_cap_mhz = mhz,
            _ => {}
        }
        s.ac_dc_reapply = s.settings.ac_dc_enabled;
        let _ = s.settings.save();
    }
    sync.wake.notify_one();
    sync.emit(&app);
}

#[tauri::command]
fn check_updates(app: AppHandle, sync: State<'_, Arc<AppSync>>) {
    spawn_update_check(Arc::clone(&sync), app, false);
}

#[tauri::command]
fn install_update(app: AppHandle, sync: State<'_, Arc<AppSync>>) {
    let info = match &sync.state.lock().unwrap().update {
        updater::UpdateState::Available(info) => info.clone(),
        _ => return,
    };
    {
        let mut s = sync.state.lock().unwrap();
        s.update = updater::UpdateState::Updating;
        s.status = format!("Downloading and verifying {}...", info.tag);
        s.is_error = false;
    }
    sync.emit(&app);

    let sync_bg = Arc::clone(&sync);
    let app_bg = app.clone();
    thread::spawn(move || match updater::download_and_install(&info) {
        Ok(()) => {
            sync_bg.relaunch_ready.store(true, Ordering::SeqCst);
            sync_bg.quitting.store(true, Ordering::SeqCst);
            app_bg.exit(0);
        }
        Err(e) => {
            {
                let mut s = sync_bg.state.lock().unwrap();
                s.update = updater::UpdateState::Error(e.clone());
                s.status = format!("Update failed: {e}");
                s.is_error = true;
            }
            sync_bg.emit(&app_bg);
        }
    });
}

fn main() {
    updater::cleanup_old_binary();
    let launched_at_startup = std::env::args().any(|a| a == "--startup");
    let app_settings = settings::AppSettings::load();
    let start_visible = !(launched_at_startup && app_settings.show_in_tray);
    let sync = AppSync::new(app_settings.clone(), launched_at_startup);

    let sync_setup = Arc::clone(&sync);
    let sync_win = Arc::clone(&sync);
    let sync_exit = Arc::clone(&sync);

    let app = tauri::Builder::default()
        .manage(Arc::clone(&sync))
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_cap,
            set_auto,
            update_setting,
            set_power_profile,
            check_updates,
            install_update,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();

            // Set window runtime icon + initial visibility
            if let Some(w) = app.get_webview_window("main") {
                let win_icon = Image::new_owned(icons::render_app_icon_rgba(64), 64, 64);
                let _ = w.set_icon(win_icon);
                if start_visible {
                    let _ = w.show();
                }
            }

            // Build native system tray with live temperature icon
            let show_i = MenuItem::with_id(app, "show", "Show kool-cpu", true, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let quiet_i = MenuItem::with_id(app, "quiet", "Quiet (2.4 GHz)", true, None::<&str>)?;
            let bal_i = MenuItem::with_id(app, "balanced", "Balanced (3.1 GHz)", true, None::<&str>)?;
            let max_i = MenuItem::with_id(app, "max", "Max (Full speed)", true, None::<&str>)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &sep1, &quiet_i, &bal_i, &max_i, &sep2, &quit_i])?;

            let sync_menu = Arc::clone(&sync_setup);
            let sync_click = Arc::clone(&sync_setup);
            let tray_img = Image::new_owned(icons::render_temp_tray_rgba(None), 32, 32);

            let tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_img)
                .tooltip("kool-cpu")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, ev| match ev.id.as_ref() {
                    "show" => show_main_window(app),
                    "quiet" => sync_menu.request_cap(app, Some(2400), true, true),
                    "balanced" => sync_menu.request_cap(app, Some(3100), true, true),
                    "max" => sync_menu.request_cap(app, None, true, true),
                    "quit" => {
                        sync_menu.quitting.store(true, Ordering::SeqCst);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(move |tray, ev| {
                    let app = tray.app_handle();
                    match ev {
                        TrayIconEvent::DoubleClick {
                            button: MouseButton::Left,
                            ..
                        } => {
                            *sync_click.last_tray_click.lock().unwrap() = Some(Instant::now());
                            show_main_window(app);
                        }
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } => {
                            let now = Instant::now();
                            let mut last = sync_click.last_tray_click.lock().unwrap();
                            let debounced = last.is_some_and(|p| now.duration_since(p) < CLICK_DEBOUNCE);
                            if !debounced {
                                *last = Some(now);
                                drop(last);
                                toggle_main_window(app);
                            }
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            if !app_settings.show_in_tray {
                let _ = tray.set_visible(false);
            }

            // Apply initial remembered speed (unless AC/Battery switching will apply immediately)
            if !app_settings.ac_dc_enabled && (launched_at_startup || !app_settings.last_auto_enabled) {
                sync_setup.request_cap(&handle, app_settings.last_cap_mhz, launched_at_startup, false);
            }

            spawn_worker(Arc::clone(&sync_setup), handle.clone());
            spawn_update_check(Arc::clone(&sync_setup), handle, true);
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let show_in_tray = sync_win.state.lock().unwrap().settings.show_in_tray;
                if show_in_tray && !sync_win.quitting.load(Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error building kool-cpu");

    app.run(move |_app, event| {
        if let RunEvent::Exit = event {
            let reset = sync_exit.state.lock().unwrap().settings.reset_on_exit;
            if reset && !sync_exit.relaunch_ready.load(Ordering::SeqCst) {
                let _ = native_backend().set_cap_mhz(None);
            }
        }
    });
}
