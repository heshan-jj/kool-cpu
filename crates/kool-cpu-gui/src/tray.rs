//! System tray icon showing live CPU temperature + quick profile menu
//! (Windows native; Linux deferred to ksni).

pub enum TrayAction {
    Show,
    SetCap(Option<u32>),
    Quit,
}

#[cfg(target_os = "windows")]
mod imp {
    use super::TrayAction;
    use crate::icons::render_temp_tray_rgba;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    pub struct Tray {
        icon: TrayIcon,
        last_temp: Option<Option<i32>>,
        last_cap: Option<Option<u32>>,
        id_show: String,
        id_quiet: String,
        id_balanced: String,
        id_max: String,
        id_quit: String,
    }

    fn icon_for(temp_c: Option<i32>) -> Result<Icon, String> {
        Icon::from_rgba(render_temp_tray_rgba(temp_c), 32, 32).map_err(|e| format!("tray icon: {e}"))
    }

    impl Tray {
        pub fn new() -> Result<Self, String> {
            let show = MenuItem::new("Show kool-cpu", true, None);
            let quiet = MenuItem::new("Quiet (2.4 GHz)", true, None);
            let balanced = MenuItem::new("Balanced (3.1 GHz)", true, None);
            let max = MenuItem::new("Max (Full speed)", true, None);
            let quit = MenuItem::new("Quit", true, None);

            let menu = Menu::new();
            let _ = menu.append_items(&[
                &show,
                &PredefinedMenuItem::separator(),
                &quiet,
                &balanced,
                &max,
                &PredefinedMenuItem::separator(),
                &quit,
            ]);

            let icon = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("kool-cpu")
                .with_icon(icon_for(None)?)
                .build()
                .map_err(|e| format!("tray build: {e}"))?;

            Ok(Self {
                icon,
                last_temp: Some(None),
                last_cap: None,
                id_show: show.id().0.clone(),
                id_quiet: quiet.id().0.clone(),
                id_balanced: balanced.id().0.clone(),
                id_max: max.id().0.clone(),
                id_quit: quit.id().0.clone(),
            })
        }

        /// Update the tray icon digits and tooltip only when rounded °C or cap changes.
        pub fn update_temp(&mut self, temp_c: Option<f32>, cap_mhz: Option<u32>) {
            let rounded = temp_c.map(|c| c.round() as i32);
            let temp_changed = self.last_temp != Some(rounded);
            if temp_changed {
                if let Ok(ic) = icon_for(rounded) {
                    let _ = self.icon.set_icon(Some(ic));
                }
                self.last_temp = Some(rounded);
            }
            if temp_changed || self.last_cap != Some(cap_mhz) {
                let temp_str = temp_c.map_or("--°C".into(), |c| format!("{c:.1}°C"));
                let cap_str = cap_mhz.map_or("Full speed".into(), |m| format!("{:.2} GHz", m as f32 / 1000.0));
                let _ = self.icon.set_tooltip(Some(format!("kool-cpu — {temp_str} | {cap_str}")));
                self.last_cap = Some(cap_mhz);
            }
        }

        pub fn poll(&self) -> Vec<TrayAction> {
            let mut actions = Vec::new();
            while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = ev
                {
                    actions.push(TrayAction::Show);
                }
            }
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                let id = ev.id.0;
                if id == self.id_show {
                    actions.push(TrayAction::Show);
                } else if id == self.id_quiet {
                    actions.push(TrayAction::SetCap(Some(2400)));
                } else if id == self.id_balanced {
                    actions.push(TrayAction::SetCap(Some(3100)));
                } else if id == self.id_max {
                    actions.push(TrayAction::SetCap(None));
                } else if id == self.id_quit {
                    actions.push(TrayAction::Quit);
                }
            }
            actions
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use super::TrayAction;

    // shortcut: Linux StatusNotifierItem tray (e.g. ksni) deferred until tested on a Linux desktop.
    pub struct Tray;

    impl Tray {
        pub fn new() -> Result<Self, String> {
            Ok(Self)
        }

        pub fn update_temp(&mut self, _temp_c: Option<f32>, _cap_mhz: Option<u32>) {}

        pub fn poll(&self) -> Vec<TrayAction> {
            Vec::new()
        }
    }
}

pub use imp::Tray;
