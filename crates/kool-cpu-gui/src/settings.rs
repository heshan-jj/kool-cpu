//! Persistent GUI settings, remembered CPU speed/profile, and OS login-startup integration.

use kool_cpu_core::AutoConfig;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_CAP_MHZ: u32 = 3100; // Balanced

#[derive(Debug, Clone, PartialEq)]
pub struct AppSettings {
    pub show_in_tray: bool,
    pub run_on_startup: bool,
    pub reset_on_exit: bool,
    /// Remembered manual/profile cap (`None` = unlimited/Max). Defaults to Balanced (3100 MHz).
    pub last_cap_mhz: Option<u32>,
    /// Whether Auto Thermal Mode was enabled on last normal session.
    pub last_auto_enabled: bool,
    pub auto_cfg: AutoConfig,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_in_tray: true,
            run_on_startup: false,
            reset_on_exit: true,
            last_cap_mhz: Some(DEFAULT_CAP_MHZ),
            last_auto_enabled: false,
            auto_cfg: AutoConfig::default(),
        }
    }
}

impl AppSettings {
    pub fn config_path() -> PathBuf {
        #[cfg(target_os = "windows")]
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        #[cfg(not(target_os = "windows"))]
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("kool-cpu").join("settings.conf")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::config_path())
    }

    pub fn load_from(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        Self::parse(&text)
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::config_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("cannot create config dir: {e}"))?;
        }
        fs::write(path, self.serialize()).map_err(|e| format!("cannot write settings: {e}"))
    }

    fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            let b = matches!(v, "true" | "1");
            match k.trim() {
                "show_in_tray" => s.show_in_tray = b,
                "run_on_startup" => s.run_on_startup = b,
                "reset_on_exit" => s.reset_on_exit = b,
                "last_cap_mhz" => {
                    s.last_cap_mhz = if v.eq_ignore_ascii_case("none") {
                        None
                    } else {
                        v.parse::<u32>().ok().filter(|&m| m >= 400).or(s.last_cap_mhz)
                    };
                }
                "last_auto_enabled" => s.last_auto_enabled = b,
                "auto_high_c" => if let Ok(x) = v.parse() { s.auto_cfg.high_c = x; },
                "auto_low_c" => if let Ok(x) = v.parse() { s.auto_cfg.low_c = x; },
                "auto_min_mhz" => if let Ok(x) = v.parse() { s.auto_cfg.min_mhz = x; },
                "auto_max_mhz" => if let Ok(x) = v.parse() { s.auto_cfg.max_mhz = x; },
                "auto_step_mhz" => if let Ok(x) = v.parse() { s.auto_cfg.step_mhz = x; },
                _ => {}
            }
        }
        if s.auto_cfg.validate().is_err() {
            s.auto_cfg = AutoConfig::default();
        }
        s
    }

    fn serialize(&self) -> String {
        let cap = match self.last_cap_mhz {
            Some(m) => m.to_string(),
            None => "none".into(),
        };
        format!(
            "show_in_tray={}\n\
             run_on_startup={}\n\
             reset_on_exit={}\n\
             last_cap_mhz={}\n\
             last_auto_enabled={}\n\
             auto_high_c={}\n\
             auto_low_c={}\n\
             auto_min_mhz={}\n\
             auto_max_mhz={}\n\
             auto_step_mhz={}\n",
            self.show_in_tray,
            self.run_on_startup,
            self.reset_on_exit,
            cap,
            self.last_auto_enabled,
            self.auto_cfg.high_c,
            self.auto_cfg.low_c,
            self.auto_cfg.min_mhz,
            self.auto_cfg.max_mhz,
            self.auto_cfg.step_mhz,
        )
    }
}

/// Enable or disable launching `kool-cpu-gui --startup` at user login.
pub fn apply_startup(enable: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate executable: {e}"))?;

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        // Task Scheduler with /RL HIGHEST is required because elevated UAC apps are skipped by HKCU\...\Run.
        let mut cmd = Command::new("schtasks");
        cmd.creation_flags(CREATE_NO_WINDOW);
        if enable {
            let tr = format!("\"{}\" --startup", exe.display());
            cmd.args(["/Create", "/TN", "kool-cpu", "/TR", &tr, "/SC", "ONLOGON", "/RL", "HIGHEST", "/F"]);
        } else {
            cmd.args(["/Delete", "/TN", "kool-cpu", "/F"]);
        }
        let out = cmd.output().map_err(|e| format!("cannot run schtasks: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if enable || !err.contains("cannot find") {
                return Err(if err.is_empty() {
                    "schtasks failed (run as Administrator?)".into()
                } else {
                    err
                });
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_string())?;
        let autostart = home.join(".config/autostart/kool-cpu.desktop");
        if enable {
            if let Some(p) = autostart.parent() {
                fs::create_dir_all(p).map_err(|e| format!("cannot create autostart dir: {e}"))?;
            }
            let entry = format!(
                "[Desktop Entry]\nType=Application\nName=kool-cpu\nExec=pkexec \"{}\" --startup\nX-GNOME-Autostart-enabled=true\n",
                exe.display()
            );
            fs::write(&autostart, entry).map_err(|e| format!("cannot write autostart entry: {e}"))?;
        } else if autostart.exists() {
            fs::remove_file(&autostart).map_err(|e| format!("cannot remove autostart entry: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn roundtrips_settings_file() {
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("kool_cpu_settings_{n}.conf"));
        let s = AppSettings {
            show_in_tray: false,
            run_on_startup: true,
            reset_on_exit: false,
            last_cap_mhz: Some(2600),
            last_auto_enabled: true,
            auto_cfg: AutoConfig {
                high_c: 82.0,
                low_c: 70.0,
                min_mhz: 2000,
                max_mhz: 3800,
                step_mhz: 200,
            },
        };
        s.save_to(&path).unwrap();
        assert_eq!(AppSettings::load_from(&path), s);

        let unlimited = AppSettings { last_cap_mhz: None, ..s };
        unlimited.save_to(&path).unwrap();
        assert_eq!(AppSettings::load_from(&path).last_cap_mhz, None);

        let _ = fs::remove_file(&path);
    }
}
