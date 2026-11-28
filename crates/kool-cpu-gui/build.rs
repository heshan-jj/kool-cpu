fn main() {
    println!("cargo:rerun-if-changed=../../packaging/windows/kool-cpu.ico");

    // Embed requireAdministrator UAC manifest on Windows MSVC release builds
    // (skip in debug/test so `cargo test --workspace` runs without elevation).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
        && std::env::var("PROFILE").as_deref() == Ok("release")
    {
        let attrs = tauri_build::Attributes::new().windows_attributes(
            tauri_build::WindowsAttributes::new().app_manifest(
                r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*" />
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#,
            ),
        );
        tauri_build::try_build(attrs).expect("failed to run tauri-build");
    } else {
        tauri_build::build();
    }
}
