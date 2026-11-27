//! System tray icon with quick profile menu (Windows native; Linux deferred to ksni).

pub enum TrayAction {
    Show,
    SetCap(Option<u32>),
    Quit,
}

#[cfg(target_os = "windows")]
mod imp {
    use super::TrayAction;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    pub struct Tray {
        _icon: TrayIcon,
        id_show: String,
        id_quiet: String,
        id_balanced: String,
        id_max: String,
        id_quit: String,
    }

    fn build_icon() -> Result<Icon, String> {
        let size = 32u32;
        let mut rgba = vec![0u8; (size * size * 4) as usize];
        let cx = 15.5f32;
        let cy = 15.5f32;
        for y in 0..size {
            for x in 0..size {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                let idx = ((y * size + x) * 4) as usize;
                if d <= 13.5 {
                    // Clean sky-blue circular badge with crisp inner dot
                    let (r, g, b) = if d <= 5.5 { (241, 243, 247) } else { (122, 184, 255) };
                    rgba[idx] = r;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b;
                    rgba[idx + 3] = 255;
                }
            }
        }
        Icon::from_rgba(rgba, size, size).map_err(|e| format!("tray icon: {e}"))
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
                .with_icon(build_icon()?)
                .build()
                .map_err(|e| format!("tray build: {e}"))?;

            Ok(Self {
                _icon: icon,
                id_show: show.id().0.clone(),
                id_quiet: quiet.id().0.clone(),
                id_balanced: balanced.id().0.clone(),
                id_max: max.id().0.clone(),
                id_quit: quit.id().0.clone(),
            })
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

        pub fn poll(&self) -> Vec<TrayAction> {
            Vec::new()
        }
    }
}

pub use imp::Tray;
