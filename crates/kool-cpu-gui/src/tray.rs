//! System tray icon showing live CPU temperature + quick profile menu
//! (Windows native; Linux deferred to ksni).

pub enum TrayAction {
    Show,
    SetCap(Option<u32>),
    Quit,
}

// Crisp 5x7 pixel font for digits 0-9 and '-' (each byte's low 5 bits are one row).
const GLYPHS: [[u8; 7]; 11] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111], // 2
    [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b01110, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
    [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000], // -
];

/// Render a 32x32 RGBA tray icon with a dark rounded backing and bold 2-digit temperature.
pub fn render_temp_rgba(temp_c: Option<i32>) -> Vec<u8> {
    const SIZE: usize = 32;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];

    let (fg, d0, d1) = match temp_c {
        Some(t) => {
            let clamped = t.clamp(0, 99) as usize;
            let col = if t >= 80 {
                (248u8, 113u8, 113u8) // hot red
            } else if t >= 70 {
                (251u8, 191u8, 36u8) // warm amber
            } else {
                (110u8, 231u8, 183u8) // cool mint
            };
            (col, clamped / 10, clamped % 10)
        }
        None => ((167u8, 176u8, 192u8), 10, 10), // "--"
    };

    // Dark rounded-square backdrop so digits stay readable on both light and dark taskbars.
    let bg = (16u8, 19u8, 24u8);
    let r = 6.0f32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let fx = (x as f32 - 15.5).abs() - (15.5 - r);
            let fy = (y as f32 - 15.5).abs() - (15.5 - r);
            let outside = (fx.max(0.0).powi(2) + fy.max(0.0).powi(2)).sqrt();
            if outside <= r {
                let idx = (y * SIZE + x) * 4;
                rgba[idx] = bg.0;
                rgba[idx + 1] = bg.1;
                rgba[idx + 2] = bg.2;
                rgba[idx + 3] = 240;
            }
        }
    }

    // Draw two 5x7 glyphs scaled 2x wide by 3x tall (10x21 each, 2px gap -> 22x21 centered).
    let draw_glyph = |rgba: &mut [u8], glyph_idx: usize, ox: usize, oy: usize| {
        let g = GLYPHS[glyph_idx];
        for (row, &bits) in g.iter().enumerate() {
            for col in 0..5 {
                if (bits & (1 << (4 - col))) != 0 {
                    for dy in 0..3 {
                        for dx in 0..2 {
                            let x = ox + col * 2 + dx;
                            let y = oy + row * 3 + dy;
                            if x < SIZE && y < SIZE {
                                let idx = (y * SIZE + x) * 4;
                                rgba[idx] = fg.0;
                                rgba[idx + 1] = fg.1;
                                rgba[idx + 2] = fg.2;
                                rgba[idx + 3] = 255;
                            }
                        }
                    }
                }
            }
        }
    };

    draw_glyph(&mut rgba, d0, 5, 5);
    draw_glyph(&mut rgba, d1, 17, 5);
    rgba
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{render_temp_rgba, TrayAction};
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
        Icon::from_rgba(render_temp_rgba(temp_c), 32, 32).map_err(|e| format!("tray icon: {e}"))
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
            if self.last_temp != Some(rounded) {
                if let Ok(ic) = icon_for(rounded) {
                    let _ = self.icon.set_icon(Some(ic));
                }
                self.last_temp = Some(rounded);
            }
            if self.last_cap != Some(cap_mhz) || self.last_temp == Some(rounded) {
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

#[cfg(test)]
mod tests {
    use super::render_temp_rgba;

    #[test]
    fn renders_temperature_icon_buffer() {
        let cool = render_temp_rgba(Some(64));
        let hot = render_temp_rgba(Some(85));
        assert_eq!(cool.len(), 32 * 32 * 4);
        // Cool icon has mint pixels (110, 231, 183); hot icon has red pixels (248, 113, 113).
        assert!(cool.chunks_exact(4).any(|p| p == [110, 231, 183, 255]));
        assert!(hot.chunks_exact(4).any(|p| p == [248, 113, 113, 255]));
    }
}
