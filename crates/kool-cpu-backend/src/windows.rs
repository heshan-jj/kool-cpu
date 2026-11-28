//! Windows backend: manages a dedicated "kool-cpu" power plan via `powercfg`
//! and reads Intel CPU package die temperature via PawnIO (`PawnIOLib.dll` + `IntelMSR.bin`).

use kool_cpu_core::{Backend, PowerSource, Telemetry};
use std::process::Command;
use std::sync::OnceLock;

const BALANCED_GUID: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
const PLAN_NAME: &str = "kool-cpu";
const SUB: &str = "SUB_PROCESSOR";
const FREQ_SETTINGS: &[&str] = &["PROCFREQMAX", "PROCFREQMAX1"];

pub struct WindowsBackend {
    cpu_name: OnceLock<String>,
}

impl WindowsBackend {
    pub fn new() -> Self {
        Self { cpu_name: OnceLock::new() }
    }
}

impl Default for WindowsBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn run(prog: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new(prog);
    cmd.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("cannot run {prog}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            err
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Find a power scheme GUID by its friendly name in `powercfg /list` output.
pub fn find_scheme_guid(list_out: &str, name: &str) -> Option<String> {
    let target = format!("({name})");
    for line in list_out.lines() {
        if line.contains(&target) {
            if let Some(after) = line.split("GUID:").nth(1) {
                if let Some(guid) = after.split_whitespace().next() {
                    return Some(guid.to_string());
                }
            }
        }
    }
    None
}

/// Extract the first `Current ... Power Setting Index` hex value from `powercfg /qh`.
pub fn parse_first_index(out: &str) -> Option<u32> {
    out.lines()
        .filter(|l| l.contains("Power Setting Index"))
        .find_map(|l| {
            let (_, v) = l.split_once(':')?;
            u32::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok()
        })
}

/// Decode Intel CPU temperature (°C) from MSR_IA32_PACKAGE_THERM_STATUS (0x1B1) or
/// MSR_IA32_THERM_STATUS (0x19C), plus MSR_IA32_TEMPERATURE_TARGET (0x1A2).
pub fn intel_dts_temp_c(therm_status: u64, temp_target: u64) -> Option<f32> {
    let tj_max = ((temp_target >> 16) & 0xFF) as i32;
    if tj_max == 0 {
        return None;
    }
    let readout = ((therm_status >> 16) & 0x7F) as i32;
    Some((tj_max - readout) as f32)
}

#[cfg(target_os = "windows")]
mod pawnio {
    use super::intel_dts_temp_c;
    use std::ffi::{c_char, c_void};
    use std::path::Path;
    use std::ptr::null_mut;

    const PAWNIO_DLL: &str = r"C:\Program Files\PawnIO\PawnIOLib.dll";
    const INTEL_MSR_BIN: &[u8] = include_bytes!("../assets/IntelMSR.bin");
    const MSR_IA32_THERM_STATUS: u64 = 0x19C;
    const MSR_IA32_TEMPERATURE_TARGET: u64 = 0x1A2;
    const MSR_IA32_PACKAGE_THERM_STATUS: u64 = 0x1B1;

    type Handle = *mut c_void;
    type HResult = i32;

    type FnOpen = unsafe extern "system" fn(*mut Handle) -> HResult;
    type FnLoad = unsafe extern "system" fn(Handle, *const u8, usize) -> HResult;
    type FnExecute = unsafe extern "system" fn(
        Handle,
        *const c_char,
        *const u64,
        usize,
        *mut u64,
        usize,
        *mut usize,
    ) -> HResult;
    type FnClose = unsafe extern "system" fn(Handle) -> HResult;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryA(lp_lib_file_name: *const c_char) -> Handle;
        fn GetProcAddress(h_module: Handle, lp_proc_name: *const c_char) -> *const c_void;
        fn FreeLibrary(h_lib_module: Handle) -> i32;
    }

    struct Session {
        lib: Handle,
        handle: Handle,
        execute: FnExecute,
        close: FnClose,
    }

    impl Session {
        fn open_intel_msr() -> Result<Self, String> {
            if !Path::new(PAWNIO_DLL).exists() {
                return Err("PawnIO not installed (install from https://pawnio.eu/)".into());
            }
            let dll_c = b"C:\\Program Files\\PawnIO\\PawnIOLib.dll\0";
            // SAFETY: dll_c is NUL-terminated; function pointers match PawnIOLib.h STDAPICALLTYPE signatures.
            unsafe {
                let lib = LoadLibraryA(dll_c.as_ptr() as *const c_char);
                if lib.is_null() {
                    return Err("failed to load PawnIOLib.dll".into());
                }
                let sym = |name: &[u8]| -> Result<*const c_void, String> {
                    let p = GetProcAddress(lib, name.as_ptr() as *const c_char);
                    if p.is_null() {
                        Err(format!("missing symbol {}", String::from_utf8_lossy(name)))
                    } else {
                        Ok(p)
                    }
                };
                let (open, load, execute, close) = match (
                    sym(b"pawnio_open\0"),
                    sym(b"pawnio_load\0"),
                    sym(b"pawnio_execute\0"),
                    sym(b"pawnio_close\0"),
                ) {
                    (Ok(o), Ok(l), Ok(e), Ok(c)) => (
                        std::mem::transmute::<_, FnOpen>(o),
                        std::mem::transmute::<_, FnLoad>(l),
                        std::mem::transmute::<_, FnExecute>(e),
                        std::mem::transmute::<_, FnClose>(c),
                    ),
                    (Err(e), _, _, _) | (_, Err(e), _, _) | (_, _, Err(e), _) | (_, _, _, Err(e)) => {
                        FreeLibrary(lib);
                        return Err(e);
                    }
                };

                let mut handle: Handle = null_mut();
                let hr = open(&mut handle);
                if hr < 0 || handle.is_null() {
                    FreeLibrary(lib);
                    return Err(format!("pawnio_open failed (HRESULT {hr:#010x}; run as Administrator?)"));
                }
                let hr = load(handle, INTEL_MSR_BIN.as_ptr(), INTEL_MSR_BIN.len());
                if hr < 0 {
                    close(handle);
                    FreeLibrary(lib);
                    return Err(format!("pawnio_load(IntelMSR.bin) failed (HRESULT {hr:#010x})"));
                }
                Ok(Self { lib, handle, execute, close })
            }
        }

        fn read_msr(&self, msr: u64) -> Result<u64, String> {
            let input = [msr];
            let mut output = [0u64];
            let mut ret_size = 0usize;
            // SAFETY: handle is open and IntelMSR.bin is loaded; buffers match ioctl_read_msr(1, 1).
            let hr = unsafe {
                (self.execute)(
                    self.handle,
                    b"ioctl_read_msr\0".as_ptr() as *const c_char,
                    input.as_ptr(),
                    1,
                    output.as_mut_ptr(),
                    1,
                    &mut ret_size,
                )
            };
            if hr < 0 || ret_size < 1 {
                return Err(format!("read_msr({msr:#x}) failed (HRESULT {hr:#010x})"));
            }
            Ok(output[0])
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            // SAFETY: handle and lib were opened in open_intel_msr.
            unsafe {
                (self.close)(self.handle);
                FreeLibrary(self.lib);
            }
        }
    }

    // shortcut: IntelMSR only for now; add AMDFamily17/RyzenSMU module when AMD hardware is tested.
    pub fn read_temp() -> (Option<f32>, String) {
        let sess = match Session::open_intel_msr() {
            Ok(s) => s,
            Err(e) => return (None, e),
        };
        let target = match sess.read_msr(MSR_IA32_TEMPERATURE_TARGET) {
            Ok(v) => v,
            Err(e) => return (None, e),
        };
        if let Ok(pkg) = sess.read_msr(MSR_IA32_PACKAGE_THERM_STATUS) {
            if let Some(c) = intel_dts_temp_c(pkg, target) {
                return (Some(c), "PawnIO:IntelMSR (package)".into());
            }
        }
        match sess.read_msr(MSR_IA32_THERM_STATUS) {
            Ok(core) => match intel_dts_temp_c(core, target) {
                Some(c) => (Some(c), "PawnIO:IntelMSR (core)".into()),
                None => (None, "PawnIO:IntelMSR returned invalid TJMax".into()),
            },
            Err(e) => (None, e),
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod pawnio {
    pub fn read_temp() -> (Option<f32>, String) {
        (None, "Windows-only".into())
    }
}

fn ensure_cooler_scheme() -> Result<String, String> {
    let list = run("powercfg", &["/list"])?;
    if let Some(guid) = find_scheme_guid(&list, PLAN_NAME) {
        return Ok(guid);
    }
    let dup = run("powercfg", &["/duplicatescheme", BALANCED_GUID])?;
    let guid = dup
        .split("GUID:")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .ok_or_else(|| format!("could not parse duplicated scheme GUID from: {dup}"))?
        .to_string();
    run("powercfg", &["/changename", &guid, PLAN_NAME, "Managed by kool-cpu"])?;
    Ok(guid)
}

fn read_cpu_name() -> String {
    let ps = "(Get-CimInstance Win32_Processor | Select-Object -First 1).Name.Trim()";
    run("powershell", &["-NoProfile", "-Command", ps])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Windows CPU".into())
}

#[cfg(target_os = "windows")]
fn read_power_source() -> PowerSource {
    #[repr(C)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemPowerStatus(lp_system_power_status: *mut SystemPowerStatus) -> i32;
    }
    let mut s = SystemPowerStatus {
        ac_line_status: 255,
        battery_flag: 0,
        battery_life_percent: 255,
        system_status_flag: 0,
        battery_life_time: u32::MAX,
        battery_full_life_time: u32::MAX,
    };
    // SAFETY: `s` is a valid repr(C) SYSTEM_POWER_STATUS buffer.
    if unsafe { GetSystemPowerStatus(&mut s) } == 0 {
        return PowerSource::Unknown;
    }
    match s.ac_line_status {
        1 => PowerSource::Ac,
        0 => PowerSource::Battery,
        _ => PowerSource::Unknown,
    }
}

#[cfg(not(target_os = "windows"))]
fn read_power_source() -> PowerSource {
    PowerSource::Unknown
}

impl Backend for WindowsBackend {
    fn telemetry(&self) -> Result<Telemetry, String> {
        let cap_mhz = run("powercfg", &["/qh", "SCHEME_CURRENT", SUB, FREQ_SETTINGS[0]])
            .ok()
            .and_then(|out| parse_first_index(&out))
            .filter(|&v| v > 0);
        let (temp_c, temp_source) = pawnio::read_temp();
        Ok(Telemetry {
            cpu_name: self.cpu_name.get_or_init(read_cpu_name).clone(),
            temp_c,
            temp_source,
            cap_mhz,
            power_source: read_power_source(),
        })
    }

    fn set_cap_mhz(&self, mhz: Option<u32>) -> Result<(), String> {
        let scheme = match mhz {
            Some(_) => ensure_cooler_scheme()?,
            None => {
                // Zero the cooler plan if it exists, then switch back to Balanced so full boost is restored.
                if let Ok(list) = run("powercfg", &["/list"]) {
                    if let Some(cooler_guid) = find_scheme_guid(&list, PLAN_NAME) {
                        for &setting in FREQ_SETTINGS {
                            for cmd in ["/setacvalueindex", "/setdcvalueindex"] {
                                let _ = run("powercfg", &[cmd, &cooler_guid, SUB, setting, "0"]);
                            }
                        }
                    }
                }
                BALANCED_GUID.to_string()
            }
        };
        let val = mhz.unwrap_or(0).to_string();
        for &setting in FREQ_SETTINGS {
            for cmd in ["/setacvalueindex", "/setdcvalueindex"] {
                if let Err(e) = run("powercfg", &[cmd, &scheme, SUB, setting, &val]) {
                    // Non-hybrid CPUs lack PROCFREQMAX1; only fail on PROCFREQMAX.
                    if setting == FREQ_SETTINGS[0] {
                        return Err(format!("powercfg failed (run as Administrator?): {e}"));
                    }
                }
            }
        }
        run("powercfg", &["/setactive", &scheme])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_scheme_guid() {
        let out = "\
Power Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced) *
Power Scheme GUID: aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee  (cooler)
";
        assert_eq!(
            find_scheme_guid(out, "cooler").as_deref(),
            Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
        );
        assert_eq!(find_scheme_guid(out, "missing"), None);
    }

    #[test]
    fn parses_hex_index() {
        let out = "  Current AC Power Setting Index: 0x00000c1c\n";
        assert_eq!(parse_first_index(out), Some(3100));
    }

    #[test]
    fn decodes_intel_dts_temp() {
        // TJMax = 100 C (0x64 << 16), readout = 37 (0x25 << 16) -> 63 C
        let target = 100u64 << 16;
        let status = (1u64 << 31) | (37u64 << 16);
        assert_eq!(intel_dts_temp_c(status, target), Some(63.0));
        assert_eq!(intel_dts_temp_c(status, 0), None);
    }
}
