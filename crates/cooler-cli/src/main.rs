//! cooler CLI:
//!   cooler status
//!   cooler set <MHz>
//!   cooler off
//!   cooler profile [quiet|balanced|max]
//!   cooler auto [--high 78] [--low 68] [--min 1800] [--max 3600] [--step 200] [--interval 3]

use cooler_backend::native_backend;
use cooler_core::{find_profile, AutoAction, AutoConfig, AutoController, Backend, BUILTIN_PROFILES};
use std::process::exit;
use std::thread::sleep;
use std::time::Duration;

fn die(msg: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("error: {msg}");
    exit(code)
}

fn fmt_cap(cap: Option<u32>) -> String {
    match cap {
        Some(m) => format!("{m} MHz"),
        None => "unlimited".into(),
    }
}

fn cmd_status(backend: &dyn Backend) {
    let t = backend.telemetry().unwrap_or_else(|e| die(e, 1));
    println!("CPU:  {}", t.cpu_name);
    match t.temp_c {
        Some(c) => println!("Temp: {c:.1} C ({})", t.temp_source),
        None => println!("Temp: unavailable ({})", t.temp_source),
    }
    println!("Cap:  {}", fmt_cap(t.cap_mhz));
}

fn parse_auto_args(args: &[&str]) -> Result<(AutoConfig, u64), String> {
    let mut cfg = AutoConfig::default();
    let mut interval = 3u64;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i];
        let val = *args.get(i + 1).ok_or_else(|| format!("missing value for {flag}"))?;
        match flag {
            "--high" => cfg.high_c = val.parse().map_err(|_| format!("invalid --high: {val}"))?,
            "--low" => cfg.low_c = val.parse().map_err(|_| format!("invalid --low: {val}"))?,
            "--min" => cfg.min_mhz = val.parse().map_err(|_| format!("invalid --min: {val}"))?,
            "--max" => cfg.max_mhz = val.parse().map_err(|_| format!("invalid --max: {val}"))?,
            "--step" => cfg.step_mhz = val.parse().map_err(|_| format!("invalid --step: {val}"))?,
            "--interval" => interval = val.parse().map_err(|_| format!("invalid --interval: {val}"))?,
            other => return Err(format!("unknown flag: {other}")),
        }
        i += 2;
    }
    if interval == 0 {
        return Err("--interval must be >= 1".into());
    }
    cfg.validate()?;
    Ok((cfg, interval))
}

fn cmd_auto(backend: &dyn Backend, args: &[&str]) {
    let (cfg, interval) = parse_auto_args(args).unwrap_or_else(|e| die(e, 2));
    let initial = backend.telemetry().unwrap_or_else(|e| die(e, 1));
    let mut ctrl = AutoController::new(cfg, initial.cap_mhz).unwrap_or_else(|e| die(e, 2));
    println!(
        "auto mode: high={:.0}C low={:.0}C range={}..{}MHz step={}MHz every {}s (Ctrl+C to stop)",
        cfg.high_c, cfg.low_c, cfg.min_mhz, cfg.max_mhz, cfg.step_mhz, interval
    );
    loop {
        match backend.telemetry() {
            Ok(t) => match ctrl.tick(backend, t.temp_c) {
                Ok(AutoAction::Throttled(mhz)) => {
                    println!("temp={:.1}C -> cap lowered to {mhz} MHz", t.temp_c.unwrap_or(0.0));
                }
                Ok(AutoAction::Relaxed(cap)) => {
                    println!("temp={:.1}C -> cap raised to {}", t.temp_c.unwrap_or(0.0), fmt_cap(cap));
                }
                Ok(AutoAction::Hold) => {}
                Ok(AutoAction::NoTemp) => {
                    eprintln!("temp unavailable ({}) — holding at {}", t.temp_source, fmt_cap(ctrl.current_cap()));
                }
                Err(e) => eprintln!("cap update failed: {e}"),
            },
            Err(e) => eprintln!("telemetry failed: {e}"),
        }
        sleep(Duration::from_secs(interval));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let backend = native_backend();

    match refs.as_slice() {
        ["status"] | ["probe"] => cmd_status(backend.as_ref()),
        ["set", mhz] => {
            let m: u32 = mhz
                .parse()
                .ok()
                .filter(|&v| v >= 400)
                .unwrap_or_else(|| die("MHz must be a number >= 400, e.g. 3100", 2));
            backend.set_cap_mhz(Some(m)).unwrap_or_else(|e| die(e, 1));
            println!("cap set to {m} MHz");
        }
        ["off"] => {
            backend.set_cap_mhz(None).unwrap_or_else(|e| die(e, 1));
            println!("cap removed (unlimited)");
        }
        ["profile"] => {
            for p in BUILTIN_PROFILES {
                println!("  {:<10} {}", p.name, fmt_cap(p.cap_mhz));
            }
        }
        ["profile", name] => {
            let p = find_profile(name).unwrap_or_else(|| die(format!("unknown profile '{name}'"), 2));
            backend.set_cap_mhz(p.cap_mhz).unwrap_or_else(|e| die(e, 1));
            println!("profile '{}' applied ({})", p.name, fmt_cap(p.cap_mhz));
        }
        ["auto", rest @ ..] => cmd_auto(backend.as_ref(), rest),
        _ => {
            eprintln!("usage: cooler status | set <MHz> | off | profile [name] | auto [flags]");
            exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_auto_args;

    #[test]
    fn parses_auto_flags() {
        let (cfg, interval) =
            parse_auto_args(&["--high", "80", "--low", "65", "--min", "2000", "--max", "3400", "--step", "100", "--interval", "5"])
                .unwrap();
        assert_eq!(cfg.high_c, 80.0);
        assert_eq!(cfg.low_c, 65.0);
        assert_eq!(cfg.min_mhz, 2000);
        assert_eq!(cfg.max_mhz, 3400);
        assert_eq!(cfg.step_mhz, 100);
        assert_eq!(interval, 5);
    }
}
