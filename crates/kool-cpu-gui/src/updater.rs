//! In-app GitHub Releases updater with SHA-256 verification and safe binary swap.
//! Uses native OS HTTPS/SHA-256/unzip tools so kool-cpu-gui stays lean with zero HTTP crates.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = "heshan-jj/kool-cpu";

#[cfg(target_os = "windows")]
const ASSET_NAME: &str = "kool-cpu-windows-x86_64.zip";
#[cfg(not(target_os = "windows"))]
const ASSET_NAME: &str = "kool-cpu-linux-x86_64.zip";

#[cfg(target_os = "windows")]
const GUI_BIN_NAME: &str = "kool-cpu-gui.exe";
#[cfg(not(target_os = "windows"))]
const GUI_BIN_NAME: &str = "kool-cpu-gui";

#[cfg(target_os = "windows")]
const CLI_BIN_NAME: &str = "kool-cpu.exe";
#[cfg(not(target_os = "windows"))]
const CLI_BIN_NAME: &str = "kool-cpu";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub tag: String,
    pub zip_url: String,
    pub checksums_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available(ReleaseInfo),
    Updating,
    Error(String),
}

/// Parse a semver string like `"v0.3.1"` or `"0.3.1-beta"` into `(major, minor, patch)`.
pub fn parse_semver(tag: &str) -> Option<(u32, u32, u32)> {
    let clean = tag.trim().trim_start_matches(['v', 'V']);
    let core = clean.split('-').next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

pub fn is_newer(latest_tag: &str, current: &str) -> bool {
    match (parse_semver(latest_tag), parse_semver(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Extract the expected lowercase hex SHA-256 digest for `filename` from `sha256sum` output.
pub fn parse_checksum(checksums_txt: &str, filename: &str) -> Option<String> {
    for line in checksums_txt.lines() {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        if name == filename && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

fn run_hidden(prog: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new(prog);
    cmd.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| format!("cannot run {prog}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("{prog} exited with {}", out.status)
        } else {
            err
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Remove any leftover `.old` executable left behind after a previous self-update.
pub fn cleanup_old_binary() {
    if let Ok(exe) = std::env::current_exe() {
        let old = old_path_for(&exe);
        if old.exists() {
            let _ = fs::remove_file(old);
        }
    }
}

fn old_path_for(exe: &Path) -> PathBuf {
    let mut s = exe.as_os_str().to_os_string();
    s.push(".old");
    PathBuf::from(s)
}

/// Query GitHub `releases/latest` and return `Some(ReleaseInfo)` if a newer release exists.
pub fn check_latest() -> Result<Option<ReleaseInfo>, String> {
    let api_url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let raw = fetch_release_fields(&api_url)?;
    let mut lines = raw.lines();
    let tag = lines.next().unwrap_or("").trim().to_string();
    let zip_url = lines.next().unwrap_or("").trim().to_string();
    let checksums_url = lines.next().unwrap_or("").trim().to_string();

    if tag.is_empty() {
        return Ok(None);
    }
    if !is_newer(&tag, CURRENT_VERSION) {
        return Ok(None);
    }
    if zip_url.is_empty() || checksums_url.is_empty() {
        return Err(format!("Release {tag} is missing {ASSET_NAME} or checksums.txt"));
    }
    Ok(Some(ReleaseInfo {
        tag,
        zip_url,
        checksums_url,
    }))
}

#[cfg(target_os = "windows")]
fn fetch_release_fields(api_url: &str) -> Result<String, String> {
    let ps = format!(
        "$r = Invoke-RestMethod -Uri '{api_url}' -Headers @{{'User-Agent'='kool-cpu'}} -ErrorAction Stop; \
         $z = ($r.assets | Where-Object {{ $_.name -eq '{ASSET_NAME}' }} | Select-Object -First 1).browser_download_url; \
         $c = ($r.assets | Where-Object {{ $_.name -eq 'checksums.txt' }} | Select-Object -First 1).browser_download_url; \
         Write-Output $r.tag_name; Write-Output $z; Write-Output $c"
    );
    match run_hidden("powershell", &["-NoProfile", "-Command", &ps]) {
        Ok(s) => Ok(s),
        // 404 means no GitHub Release has been published yet -> treat as up to date.
        Err(e) if e.contains("404") || e.contains("Not Found") => Ok(String::new()),
        Err(e) => Err(e),
    }
}

#[cfg(not(target_os = "windows"))]
fn fetch_release_fields(api_url: &str) -> Result<String, String> {
    let body = run_hidden(
        "curl",
        &["-fsSL", "-H", "User-Agent: kool-cpu", api_url],
    );
    let json = match body {
        Ok(s) => s,
        Err(e) if e.contains("404") => return Ok(String::new()),
        Err(e) => return Err(e),
    };
    let extract = |key: &str| -> String {
        let needle = format!("\"{key}\"");
        json.split(&needle)
            .nth(1)
            .and_then(|s| s.split('"').nth(1))
            .unwrap_or("")
            .to_string()
    };
    let tag = extract("tag_name");
    let find_url = |asset: &str| -> String {
        json.split("\"browser_download_url\"")
            .skip(1)
            .filter_map(|s| s.split('"').nth(1))
            .find(|u| u.ends_with(&format!("/{asset}")))
            .unwrap_or("")
            .to_string()
    };
    Ok(format!("{tag}\n{}\n{}", find_url(ASSET_NAME), find_url("checksums.txt")))
}

/// Download the release zip + checksums.txt, verify SHA-256, swap binaries, and spawn the new exe.
pub fn download_and_install(info: &ReleaseInfo) -> Result<(), String> {
    if !info.zip_url.starts_with("https://github.com/")
        || !info.checksums_url.starts_with("https://github.com/")
    {
        return Err("Refusing to download from non-GitHub HTTPS URL".into());
    }

    let tmp = std::env::temp_dir().join(format!("kool_cpu_update_{}", info.tag));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).map_err(|e| format!("cannot create temp dir: {e}"))?;

    let zip_path = tmp.join(ASSET_NAME);
    let sum_path = tmp.join("checksums.txt");
    let extract_dir = tmp.join("extracted");

    download_file(&info.checksums_url, &sum_path)?;
    download_file(&info.zip_url, &zip_path)?;

    let checksums_txt = fs::read_to_string(&sum_path)
        .map_err(|e| format!("cannot read checksums.txt: {e}"))?;
    let expected = parse_checksum(&checksums_txt, ASSET_NAME)
        .ok_or_else(|| format!("checksums.txt does not list a valid SHA-256 for {ASSET_NAME}"))?;
    let actual = sha256_file(&zip_path)?;
    if actual != expected {
        let _ = fs::remove_dir_all(&tmp);
        return Err(format!("SHA-256 mismatch (expected {expected}, got {actual})"));
    }

    extract_zip(&zip_path, &extract_dir)?;

    let new_gui = extract_dir.join(GUI_BIN_NAME);
    if !new_gui.exists() {
        let _ = fs::remove_dir_all(&tmp);
        return Err(format!("release archive did not contain {GUI_BIN_NAME}"));
    }

    let cur_exe = std::env::current_exe().map_err(|e| format!("cannot locate current exe: {e}"))?;
    let old_exe = old_path_for(&cur_exe);
    let _ = fs::remove_file(&old_exe);

    // Rename running exe -> .old, copy new verified exe into its place.
    fs::rename(&cur_exe, &old_exe)
        .map_err(|e| format!("cannot stage current binary (check permissions): {e}"))?;
    if let Err(e) = fs::copy(&new_gui, &cur_exe) {
        // Roll back so the user is never left without a binary.
        let _ = fs::rename(&old_exe, &cur_exe);
        return Err(format!("cannot install new binary: {e}"));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&cur_exe, fs::Permissions::from_mode(0o755));
    }

    // Also update kool-cpu CLI if it lives next to kool-cpu-gui.
    let new_cli = extract_dir.join(CLI_BIN_NAME);
    if let Some(dir) = cur_exe.parent() {
        let cur_cli = dir.join(CLI_BIN_NAME);
        if new_cli.exists() && cur_cli.exists() {
            let _ = fs::copy(&new_cli, &cur_cli);
        }
    }

    let _ = fs::remove_dir_all(&tmp);

    // Spawn the newly installed binary.
    Command::new(&cur_exe)
        .spawn()
        .map_err(|e| format!("updated binary installed, but relaunch failed: {e}"))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn download_file(url: &str, dest: &Path) -> Result<(), String> {
    let ps = format!(
        "Invoke-WebRequest -Uri '{url}' -OutFile '{}' -Headers @{{'User-Agent'='kool-cpu'}} -ErrorAction Stop",
        dest.display()
    );
    run_hidden("powershell", &["-NoProfile", "-Command", &ps]).map(|_| ())
}

#[cfg(not(target_os = "windows"))]
fn download_file(url: &str, dest: &Path) -> Result<(), String> {
    run_hidden(
        "curl",
        &["-fsSL", "-H", "User-Agent: kool-cpu", "-o", &dest.to_string_lossy(), url],
    )
    .map(|_| ())
}

#[cfg(target_os = "windows")]
fn sha256_file(path: &Path) -> Result<String, String> {
    let ps = format!("(Get-FileHash -Algorithm SHA256 -Path '{}').Hash.ToLower()", path.display());
    run_hidden("powershell", &["-NoProfile", "-Command", &ps]).map(|s| s.trim().to_string())
}

#[cfg(not(target_os = "windows"))]
fn sha256_file(path: &Path) -> Result<String, String> {
    let out = run_hidden("sha256sum", &[&path.to_string_lossy()])?;
    out.split_whitespace()
        .next()
        .map(|s| s.to_ascii_lowercase())
        .ok_or_else(|| "unexpected sha256sum output".into())
}

#[cfg(target_os = "windows")]
fn extract_zip(zip: &Path, dest: &Path) -> Result<(), String> {
    let ps = format!(
        "Expand-Archive -Path '{}' -DestinationPath '{}' -Force -ErrorAction Stop",
        zip.display(),
        dest.display()
    );
    run_hidden("powershell", &["-NoProfile", "-Command", &ps]).map(|_| ())
}

#[cfg(not(target_os = "windows"))]
fn extract_zip(zip: &Path, dest: &Path) -> Result<(), String> {
    run_hidden("unzip", &["-o", &zip.to_string_lossy(), "-d", &dest.to_string_lossy()]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_semver_tags() {
        assert!(is_newer("v0.3.0", "0.2.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(!is_newer("v0.2.0", "0.2.0"));
        assert!(!is_newer("v0.1.9", "0.2.0"));
        assert_eq!(parse_semver("v0.2.1-rc1"), Some((0, 2, 1)));
    }

    #[test]
    fn parses_sha256_checksums() {
        let txt = "\
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  kool-cpu-windows-x86_64.zip
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef *kool-cpu-linux-x86_64.zip
";
        assert_eq!(
            parse_checksum(txt, "kool-cpu-windows-x86_64.zip").as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        assert_eq!(
            parse_checksum(txt, "kool-cpu-linux-x86_64.zip").as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(parse_checksum(txt, "missing.zip"), None);
    }
}
