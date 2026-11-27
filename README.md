# kool-cpu

An open-source, cross-platform CPU frequency limiter and thermal manager written in Rust for **Windows** and **Linux**.

`kool-cpu` helps keep laptops cool and quiet by capping maximum CPU clock frequency on demand or automatically stepping the cap down/up based on real CPU die temperature. It is an independent open-source project built from scratch.

## Features

- **Material 3 GUI (`kool-cpu-gui`):** live CPU package temperature, 2-minute rolling history graph with hover readout, quick power profiles (`Quiet`, `Balanced`, `Max`), manual frequency slider, Auto Thermal Mode, and a Windows system tray icon (minimize to tray on close + quick profile switching).
- **Auto-elevation:** Windows GUI embeds a UAC `requireAdministrator` manifest so double-clicking prompts for Admin automatically; Linux includes a `pkexec` `.desktop` entry and Polkit policy under `packaging/linux/`.
- **CLI (`kool-cpu`):** `status`, `set <MHz>`, `off`, `profile [name]`, and `auto` for scripting or headless use.
- **Windows backend:** manages a dedicated `"kool-cpu"` Windows power plan (`PROCFREQMAX` + `PROCFREQMAX1` for hybrid P/E-core CPUs) and reads real Intel CPU package die temperatures via [PawnIO](https://pawnio.eu/).
- **Linux backend:** caps `scaling_max_freq` under `/sys/devices/system/cpu/cpufreq/policy*` and reads CPU temperatures from `/sys/class/hwmon` (`coretemp`, `k10temp`, `zenpower`) with `thermal_zone` fallback.

## Workspace Layout

- `crates/kool-cpu-core` — `Backend` trait, profiles, and pure-Rust hysteresis `AutoController`
- `crates/kool-cpu-backend` — Windows (`powercfg` + PawnIO) and Linux (`sysfs` + `hwmon`) backends
- `crates/kool-cpu-cli` — command-line interface (`kool-cpu`)
- `crates/kool-cpu-gui` — `egui`/`eframe` (`glow`) desktop app (`kool-cpu-gui`)
- `packaging/linux/` — `.desktop` launcher and Polkit (`pkexec`) policy

## Requirements

- **Windows:**
  - Install [PawnIO](https://pawnio.eu/) for live CPU die temperatures.
  - `kool-cpu-gui.exe` requests Administrator via UAC automatically; run `kool-cpu.exe` from an Administrator terminal.
- **Linux:**
  - Run with `root` / `sudo`, or install `packaging/linux/kool-cpu.desktop` + `org.koolcpu.policy` for `pkexec`.

## Build & Run

```bash
# Run all unit tests
cargo test --workspace

# Build optimized release binaries
cargo build --release --workspace

# Launch GUI (Windows prompts for UAC automatically; Linux: sudo or pkexec)
./target/release/kool-cpu-gui

# Or use the CLI
./target/release/kool-cpu status
./target/release/kool-cpu set 3100
./target/release/kool-cpu profile quiet
./target/release/kool-cpu auto --high 78 --low 68 --min 1800 --max 3600
./target/release/kool-cpu off
```

## Releases

Push a version tag (e.g. `git tag v0.2.0 && git push origin v0.2.0`) to trigger GitHub Actions to build and publish Windows and Linux release zips automatically.

## Third-Party Attributions

- `crates/kool-cpu-backend/assets/IntelMSR.bin` is from [namazso/PawnIO.Modules](https://github.com/namazso/PawnIO.Modules), licensed under **LGPL-2.1** (see `crates/kool-cpu-backend/assets/COPYING.PawnIO.Modules`).

## License

MIT — see [LICENSE](LICENSE).
