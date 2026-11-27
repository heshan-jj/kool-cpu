//! Platform backends for Windows and Linux.
//! Both modules compile on every target so their parsers and sysfs logic stay unit-tested everywhere.

pub mod linux;
pub mod windows;

use kool_cpu_core::Backend;

pub fn native_backend() -> Box<dyn Backend> {
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsBackend::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxBackend::new())
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        compile_error!("kool-cpu-backend supports Windows and Linux");
    }
}
