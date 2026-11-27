# In-App 1-Click Self-Updater — Short Design

## Intent & Why
Users running `kool-cpu-gui` shouldn't have to manually check GitHub, download zips, and overwrite binaries when a new release ships. They should see that an update is available and apply it in one click from inside the app.

## Goals, Non-Goals & Anti-Goals
- **Goals:**
  - Automatically check `heshan-jj/kool-cpu` GitHub Releases once in the background at launch, plus a manual **Check for updates** button in Settings.
  - Show a subtle indicator dot on the Settings icon and an **Update to vX.Y.Z** button in Settings when a newer release exists.
  - Download, verify SHA-256 checksum, replace the binary in-place (plus `kool-cpu` CLI if present in the same folder), and relaunch cleanly.
- **Non-Goals:**
  - Silent background auto-installation without user consent.
  - Delta/binary-diff patching or Windows MSI/NSIS installers.
- **Anti-Goals:**
  - Never execute a downloaded binary whose SHA-256 does not match the release's `checksums.txt` (critical because `kool-cpu-gui.exe` runs elevated).
  - Never bloat the 5.8 MB release binary with heavy async HTTP/TLS stacks (`tokio`/`reqwest`).

## Constraints
- **Windows locked `.exe`:** Windows forbids overwriting a running executable, but allows renaming it. The updater renames the running `.exe` to `kool-cpu-gui.exe.old`, moves the verified new `.exe` into its place, spawns it, and exits. On every launch, `kool-cpu-gui` deletes any leftover `.old` file.
- **Release assets:** `.github/workflows/ci.yml` will generate and attach `checksums.txt` (SHA-256 for `kool-cpu-windows-x86_64.zip` and `kool-cpu-linux-x86_64.zip`) to every GitHub Release.
- **Zero new Rust crates:** Use native OS HTTPS + SHA-256 + unzip tools (`powershell` `Invoke-RestMethod` / `Get-FileHash` / `Expand-Archive` with `CREATE_NO_WINDOW` on Windows; `curl` / `sha256sum` / `unzip` on Linux).

## Approaches Considered
1. **In-app 1-click self-updater (Chosen):** Best UX — users click once and relaunch into the new version, with SHA-256 verification and `.old` swap.
2. **Notifier + open browser:** Simpler, but forces manual unzip and file replacement.
3. **Package manager (`winget` / `scoop`):** Good later addition, but requires manifest PRs per release and doesn't help portable `.exe` users.

## User Flow & UI
1. On startup, `kool-cpu-gui` deletes any `*.old` leftover and runs one background check against `https://api.github.com/repos/heshan-jj/kool-cpu/releases/latest`.
2. If `latest_tag > CARGO_PKG_VERSION`, a small primary-blue dot appears on the **Settings** icon in the top bar.
3. In **Settings**, a new **Updates** card shows current version (`v0.2.0`), status (`Up to date` / `vX.Y.Z available`), a **Check** button, and—when an update exists—an **Update now** pill button.
4. Clicking **Update now** downloads the platform zip + `checksums.txt` into a temp dir, verifies SHA-256, swaps the binary, and relaunches. Any failure (offline, rate limit, checksum mismatch) leaves the current binary untouched and shows a clear message in the status bar.

## Left to the Builder
- Semver comparison (`parse_semver`) and `checksums.txt` parser with unit tests.
- Clean cancellation/error handling and ensuring `reset_on_exit` still behaves predictably during relaunch.
