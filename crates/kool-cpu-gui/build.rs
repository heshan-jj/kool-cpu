fn main() {
    // Embed requireAdministrator UAC manifest in Windows MSVC release builds so
    // double-clicking kool-cpu-gui.exe prompts for Admin automatically while
    // `cargo test --workspace` still runs without elevation.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
        && std::env::var("PROFILE").as_deref() == Ok("release")
    {
        println!("cargo:rustc-link-arg-bin=kool-cpu-gui=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bin=kool-cpu-gui=/MANIFESTUAC:level='requireAdministrator'");
    }
}
