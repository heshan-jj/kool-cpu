//! Persistent GUI settings and OS login-startup integration (Windows Task Scheduler / Linux XDG autostart).

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSettings {
    pub show_in_tray: bool,
    pub run_on_startup: bool,
    pub reset_on_exit: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_in_tray: true,
            run_on_startup: false,
            reset_on_exit: true,
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
            let val = matches!(v.trim(), "true" | "1");
            match k.trim() {
                "show_in_tray" => s.show_in_tray = val,
                "run_on_startup" => s.run_on_startup = val,
                "reset_on_exit" => s.reset_on_exit = val,
                _ => {}
            }
        }
        s
    }

    fn serialize(&self) -> String {
        format!(
            "show_in_tray={}\nrun_on_startup={}\nreset_on_exit={}\n",
            self.show_in_tray, self.run_on_startup, self.reset_on_exit
        )
    }
}

/// Enable or disable launching `kool-cpu-gui --startup` (coolmode off) at user login.
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
            // Deleting a task that doesn't exist is fine.
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
        };
        s.save_to(&path).unwrap();
        assert_eq!(AppSettings::load_from(&path), s);
        let _ = fs::remove_file(&path);
    }
}
