//! Linux backend: caps CPU frequency via sysfs cpufreq policies and reads
//! CPU temperature from hwmon (coretemp/k10temp/zenpower) or thermal_zone.

use kool_cpu_core::{Backend, Telemetry};
use std::fs;
use std::path::{Path, PathBuf};

const PREFERRED_HWMON: &[&str] = &["coretemp", "k10temp", "zenpower", "cpu_thermal"];

pub struct LinuxBackend {
    sys_root: PathBuf,
}

impl LinuxBackend {
    pub fn new() -> Self {
        Self { sys_root: PathBuf::from("/sys") }
    }

    pub fn with_sys_root(sys_root: impl Into<PathBuf>) -> Self {
        Self { sys_root: sys_root.into() }
    }

    fn policy_dirs(&self) -> Result<Vec<PathBuf>, String> {
        let dir = self.sys_root.join("devices/system/cpu/cpufreq");
        let entries = fs::read_dir(&dir)
            .map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        let mut policies: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("policy"))
            })
            .collect();
        policies.sort();
        Ok(policies)
    }

    fn read_hwmon_temp(&self) -> Option<(f32, String)> {
        let hwmon_dir = self.sys_root.join("class/hwmon");
        let entries = fs::read_dir(hwmon_dir).ok()?;
        let mut best: Option<(usize, f32, String)> = None;

        for entry in entries.flatten() {
            let path = entry.path();
            let name = fs::read_to_string(path.join("name")).ok()?.trim().to_string();
            let priority = PREFERRED_HWMON
                .iter()
                .position(|&p| p == name)
                .unwrap_or(PREFERRED_HWMON.len());

            // Pick the hottest temp*_input in this hwmon device (package or hottest core).
            let max_c = fs::read_dir(&path)
                .ok()?
                .flatten()
                .filter_map(|f| {
                    let fname = f.file_name().to_str()?.to_string();
                    if fname.starts_with("temp") && fname.ends_with("_input") {
                        let millic: f32 = fs::read_to_string(f.path()).ok()?.trim().parse().ok()?;
                        Some(millic / 1000.0)
                    } else {
                        None
                    }
                })
                .reduce(f32::max);

            if let Some(temp_c) = max_c {
                let replace = match &best {
                    None => true,
                    Some((best_pri, _, _)) => priority < *best_pri,
                };
                if replace {
                    best = Some((priority, temp_c, format!("hwmon:{name}")));
                }
            }
        }
        best.map(|(_, t, src)| (t, src))
    }

    fn read_thermal_zone_temp(&self) -> Option<(f32, String)> {
        let tz_dir = self.sys_root.join("class/thermal");
        let max_c = fs::read_dir(tz_dir)
            .ok()?
            .flatten()
            .filter_map(|e| {
                let fname = e.file_name().to_str()?.to_string();
                if !fname.starts_with("thermal_zone") {
                    return None;
                }
                let millic: f32 = fs::read_to_string(e.path().join("temp")).ok()?.trim().parse().ok()?;
                Some(millic / 1000.0)
            })
            .reduce(f32::max)?;
        Some((max_c, "sysfs:thermal_zone".into()))
    }
}

impl Default for LinuxBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn read_u32(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_cpu_name() -> String {
    fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_else(|| "Linux CPU".into())
}

impl Backend for LinuxBackend {
    fn telemetry(&self) -> Result<Telemetry, String> {
        let cap_mhz = self.policy_dirs().ok().and_then(|dirs| {
            let p = dirs.first()?;
            let cur_khz = read_u32(&p.join("scaling_max_freq"))?;
            let hw_max_khz = read_u32(&p.join("cpuinfo_max_freq"));
            if hw_max_khz.is_some_and(|max| cur_khz >= max) {
                None
            } else {
                Some(cur_khz / 1000)
            }
        });

        let (temp_c, temp_source) = self
            .read_hwmon_temp()
            .or_else(|| self.read_thermal_zone_temp())
            .map(|(t, s)| (Some(t), s))
            .unwrap_or_else(|| (None, "no hwmon/thermal sensor found".into()));

        Ok(Telemetry {
            cpu_name: read_cpu_name(),
            temp_c,
            temp_source,
            cap_mhz,
        })
    }

    fn set_cap_mhz(&self, mhz: Option<u32>) -> Result<(), String> {
        let policies = self.policy_dirs()?;
        if policies.is_empty() {
            return Err("no cpufreq policies found under sysfs".into());
        }
        for policy in policies {
            let min_khz = read_u32(&policy.join("cpuinfo_min_freq")).unwrap_or(400_000);
            let max_khz = read_u32(&policy.join("cpuinfo_max_freq")).unwrap_or(u32::MAX);
            let target_khz = match mhz {
                Some(m) => m.saturating_mul(1000).clamp(min_khz, max_khz),
                None => max_khz,
            };
            let target_file = policy.join("scaling_max_freq");
            fs::write(&target_file, format!("{target_khz}\n"))
                .map_err(|e| format!("cannot write {} (run as root?): {e}", target_file.display()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("cooler_linux_test_{tag}_{n}"));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_hwmon_and_writes_clamped_cap() {
        let root = temp_dir("cap");
        let policy = root.join("devices/system/cpu/cpufreq/policy0");
        let hwmon0 = root.join("class/hwmon/hwmon0");
        let hwmon1 = root.join("class/hwmon/hwmon1");
        fs::create_dir_all(&policy).unwrap();
        fs::create_dir_all(&hwmon0).unwrap();
        fs::create_dir_all(&hwmon1).unwrap();

        fs::write(policy.join("cpuinfo_min_freq"), "800000\n").unwrap();
        fs::write(policy.join("cpuinfo_max_freq"), "4600000\n").unwrap();
        fs::write(policy.join("scaling_max_freq"), "4600000\n").unwrap();

        // hwmon0 is a non-CPU sensor, hwmon1 is coretemp -> coretemp must win.
        fs::write(hwmon0.join("name"), "acpitz\n").unwrap();
        fs::write(hwmon0.join("temp1_input"), "90000\n").unwrap();
        fs::write(hwmon1.join("name"), "coretemp\n").unwrap();
        fs::write(hwmon1.join("temp1_input"), "63500\n").unwrap();

        let b = LinuxBackend::with_sys_root(&root);
        let t = b.telemetry().unwrap();
        assert_eq!(t.cap_mhz, None);
        assert_eq!(t.temp_source, "hwmon:coretemp");
        assert!((t.temp_c.unwrap() - 63.5).abs() < 1e-4);

        b.set_cap_mhz(Some(3100)).unwrap();
        assert_eq!(fs::read_to_string(policy.join("scaling_max_freq")).unwrap().trim(), "3100000");
        assert_eq!(b.telemetry().unwrap().cap_mhz, Some(3100));

        // Removing cap restores cpuinfo_max_freq
        b.set_cap_mhz(None).unwrap();
        assert_eq!(fs::read_to_string(policy.join("scaling_max_freq")).unwrap().trim(), "4600000");

        let _ = fs::remove_dir_all(&root);
    }
}
