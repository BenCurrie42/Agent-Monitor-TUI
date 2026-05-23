// On macOS with a nix-managed Rust toolchain the SDK library path is not on the
// linker search list by default, so linking against libiconv (pulled in by
// `notify`/`fsevent-sys` transitively) fails. Auto-discover the SDK lib path
// via `xcrun` and add it.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("xcrun")
            .arg("--show-sdk-path")
            .output()
        {
            if out.status.success() {
                let sdk = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !sdk.is_empty() {
                    println!("cargo:rustc-link-search=native={sdk}/usr/lib");
                }
            }
        }
    }
}
