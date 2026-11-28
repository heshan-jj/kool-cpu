//! Core types, profiles, and temperature-driven hysteresis controller.
//! Pure Rust with zero OS dependencies so it can be unit-tested anywhere.

/// System power source (AC adapter vs. battery).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSource {
    Ac,
    Battery,
    Unknown,
}

/// Hardware snapshot returned by a platform backend.
#[derive(Debug, Clone, PartialEq)]
pub struct Telemetry {
    pub cpu_name: String,
    pub temp_c: Option<f32>,
    pub temp_source: String,
    /// Current cap in MHz, or `None` when unlimited.
    pub cap_mhz: Option<u32>,
    pub power_source: PowerSource,
}

/// Platform abstraction implemented by Windows and Linux backends.
pub trait Backend {
    fn telemetry(&self) -> Result<Telemetry, String>;
    /// Set max CPU frequency cap in MHz, or `None` to remove the cap.
    fn set_cap_mhz(&self, mhz: Option<u32>) -> Result<(), String>;
}

/// Named frequency cap preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub name: &'static str,
    pub cap_mhz: Option<u32>,
}

pub const BUILTIN_PROFILES: &[Profile] = &[
    Profile { name: "quiet", cap_mhz: Some(2400) },
    Profile { name: "balanced", cap_mhz: Some(3100) },
    Profile { name: "max", cap_mhz: None },
];

pub fn find_profile(name: &str) -> Option<Profile> {
    BUILTIN_PROFILES
        .iter()
        .copied()
        .find(|p| p.name.eq_ignore_ascii_case(name))
}

/// Settings for temperature-driven auto-capping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoConfig {
    pub high_c: f32,
    pub low_c: f32,
    pub min_mhz: u32,
    pub max_mhz: u32,
    pub step_mhz: u32,
}

impl Default for AutoConfig {
    fn default() -> Self {
        Self {
            high_c: 78.0,
            low_c: 68.0,
            min_mhz: 1800,
            max_mhz: 3600,
            step_mhz: 200,
        }
    }
}

impl AutoConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.low_c >= self.high_c {
            return Err(format!("low_c ({}) must be < high_c ({})", self.low_c, self.high_c));
        }
        if self.min_mhz < 400 || self.min_mhz >= self.max_mhz {
            return Err(format!("invalid MHz range: {}..{}", self.min_mhz, self.max_mhz));
        }
        if self.step_mhz == 0 {
            return Err("step_mhz must be > 0".into());
        }
        Ok(())
    }
}

/// Action taken by one tick of `AutoController`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoAction {
    /// Lowered the cap to cool down.
    Throttled(u32),
    /// Raised the cap (or removed it when reaching `max_mhz`) because temps are cool.
    Relaxed(Option<u32>),
    /// Temperature is in the deadband or already at the bound; no change needed.
    Hold,
    /// Backend could not report a temperature.
    NoTemp,
}

/// Stateful hysteresis controller that steps the CPU cap down when hot and up when cool.
pub struct AutoController {
    cfg: AutoConfig,
    current_cap: Option<u32>,
}

impl AutoController {
    pub fn new(cfg: AutoConfig, initial_cap: Option<u32>) -> Result<Self, String> {
        cfg.validate()?;
        Ok(Self { cfg, current_cap: initial_cap })
    }

    pub fn current_cap(&self) -> Option<u32> {
        self.current_cap
    }

    /// Compute the next cap for a measured temperature and apply it to `backend` if it changed.
    pub fn tick<B: Backend + ?Sized>(&mut self, backend: &B, temp_c: Option<f32>) -> Result<AutoAction, String> {
        let Some(t) = temp_c else {
            return Ok(AutoAction::NoTemp);
        };

        if t >= self.cfg.high_c {
            let base = self.current_cap.unwrap_or(self.cfg.max_mhz);
            let next = base.saturating_sub(self.cfg.step_mhz).max(self.cfg.min_mhz);
            if self.current_cap == Some(next) {
                return Ok(AutoAction::Hold);
            }
            backend.set_cap_mhz(Some(next))?;
            self.current_cap = Some(next);
            Ok(AutoAction::Throttled(next))
        } else if t <= self.cfg.low_c {
            let Some(cur) = self.current_cap else {
                return Ok(AutoAction::Hold);
            };
            let raised = cur.saturating_add(self.cfg.step_mhz);
            let next = if raised >= self.cfg.max_mhz { None } else { Some(raised) };
            backend.set_cap_mhz(next)?;
            self.current_cap = next;
            Ok(AutoAction::Relaxed(next))
        } else {
            Ok(AutoAction::Hold)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeBackend {
        applied: RefCell<Vec<Option<u32>>>,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self { applied: RefCell::new(Vec::new()) }
        }
    }

    impl Backend for FakeBackend {
        fn telemetry(&self) -> Result<Telemetry, String> {
            Ok(Telemetry {
                cpu_name: "FakeCPU".into(),
                temp_c: Some(60.0),
                temp_source: "fake".into(),
                cap_mhz: *self.applied.borrow().last().unwrap_or(&None),
                power_source: PowerSource::Ac,
            })
        }

        fn set_cap_mhz(&self, mhz: Option<u32>) -> Result<(), String> {
            self.applied.borrow_mut().push(mhz);
            Ok(())
        }
    }

    #[test]
    fn hysteresis_throttles_holds_and_relaxes() {
        let cfg = AutoConfig {
            high_c: 80.0,
            low_c: 70.0,
            min_mhz: 2000,
            max_mhz: 3200,
            step_mhz: 400,
        };
        let b = FakeBackend::new();
        let mut ctrl = AutoController::new(cfg, None).unwrap();

        // Hot -> drops from max (3200) by 400 to 2800
        assert_eq!(ctrl.tick(&b, Some(82.0)).unwrap(), AutoAction::Throttled(2800));
        // Deadband (75 C) -> holds at 2800 without calling backend
        assert_eq!(ctrl.tick(&b, Some(75.0)).unwrap(), AutoAction::Hold);
        // Still hot -> 2400 -> 2000 -> holds at min
        assert_eq!(ctrl.tick(&b, Some(81.0)).unwrap(), AutoAction::Throttled(2400));
        assert_eq!(ctrl.tick(&b, Some(81.0)).unwrap(), AutoAction::Throttled(2000));
        assert_eq!(ctrl.tick(&b, Some(85.0)).unwrap(), AutoAction::Hold);
        // Cool -> steps back up until unlimited
        assert_eq!(ctrl.tick(&b, Some(65.0)).unwrap(), AutoAction::Relaxed(Some(2400)));
        assert_eq!(ctrl.tick(&b, Some(65.0)).unwrap(), AutoAction::Relaxed(Some(2800)));
        assert_eq!(ctrl.tick(&b, Some(65.0)).unwrap(), AutoAction::Relaxed(None));
        assert_eq!(ctrl.tick(&b, Some(65.0)).unwrap(), AutoAction::Hold);

        assert_eq!(
            *b.applied.borrow(),
            vec![Some(2800), Some(2400), Some(2000), Some(2400), Some(2800), None]
        );
    }

    #[test]
    fn missing_temp_does_not_touch_backend() {
        let b = FakeBackend::new();
        let mut ctrl = AutoController::new(AutoConfig::default(), None).unwrap();
        assert_eq!(ctrl.tick(&b, None).unwrap(), AutoAction::NoTemp);
        assert!(b.applied.borrow().is_empty());
    }

    #[test]
    fn rejects_invalid_config() {
        let bad = AutoConfig { high_c: 70.0, low_c: 75.0, ..Default::default() };
        assert!(AutoController::new(bad, None).is_err());
    }
}
