# AC / Battery Auto-Profiles & Tray Click Fix — Short Design

## Intent & Why
Laptop users want cool, quiet battery life when unplugged and stronger performance when plugged in, without manually switching profiles every time they attach or remove their charger. At the same time, clicking or double-clicking the Windows system tray icon should reliably show/hide the window without double-firing or getting stuck minimized.

## Goals, Non-Goals & Anti-Goals
- **Goals:**
  - Detect live power source (`Ac`, `Battery`, `Unknown`) on both Windows (`kernel32::GetSystemPowerStatus`, zero new crates) and Linux (`/sys/class/power_supply/*/online`).
  - Let users enable **Plugged-in / Battery auto-profiles** and choose a profile (`Quiet 2.4 GHz`, `Balanced 3.1 GHz`, or `Max (Full)`) for **Plugged in** (default: `Balanced`) and **On battery** (default: `Quiet`).
  - Automatically apply the matching profile whenever the charger is plugged in or unplugged (and on startup when enabled), showing the live power source in the GUI and status bar.
  - Fix the Windows tray icon click bug: reliably restore hidden *and* minimized windows (`Visible(true)` + `Minimized(false)` + `Focus`) and debounce rapid/double clicks so double-clicking never flickers or immediately hides the window.
- **Non-Goals:**
  - Battery percentage thresholds (e.g., "below 20% switch to 1.8 GHz") — keep it simple with AC vs. Battery first.
  - Changing Windows display brightness or sleep timers.
- **Anti-Goals:**
  - Fighting manual overrides: if the user manually clicks a profile or enables Auto Thermal Mode while AC/Battery switching is on, respect their manual change until the next actual `Ac ↔ Battery` plug/unplug transition.

## Approaches Considered
1. **App-managed live `PowerSource` detection (Chosen):** Backend exposes `power_source()` via `GetSystemPowerStatus` (Windows) and sysfs `power_supply` (Linux). Worker detects transitions on every 2s poll, applies the configured profile, and updates UI telemetry. Works on both OSes, supports `Max (Full)` cleanly, and displays live AC/Battery status.
2. **Windows-only `powercfg` `/setacvalueindex` vs `/setdcvalueindex`:** Simpler on paper, but doesn't work on Linux, gives no UI visibility into power state, and breaks when AC is `Max (Full)` (which switches to the `Balanced` plan).

## UI & Persistence
- **Persistence (`settings.conf`):**
  - `ac_dc_enabled=false|true` (default: `false`)
  - `ac_cap_mhz=3100|2400|none` (default: `3100` / Balanced)
  - `dc_cap_mhz=2400|3100|none` (default: `2400` / Quiet)
- **Settings page:** New **Power source switching** row with toggle switch; when ON, reveals two compact profile rows (**Plugged in** and **On battery**) with `Quiet` / `Balanced` / `Max` tonal pills.
- **Monitor view:** Shows `Plugged in` or `On battery` next to the cap/source readout when detected (omitted on desktops returning `Unknown`).
- **Tray click behavior:**
  - Single left-click toggles window visibility (or restores + focuses if minimized/hidden).
  - Ignores `TrayIconEvent::DoubleClick` and debounces left-click within 350ms so double-clicking always leaves the window cleanly open and focused instead of open-then-close.

## Left to the Builder
- Unit tests for `PowerSource` parsing (Linux sysfs fake dir), `AppSettings` roundtrip for `ac_dc_*` fields, and transition detection logic.
