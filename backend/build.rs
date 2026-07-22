// rust-embed's `#[folder = "web-dist"]` requires the directory to exist at
// compile time. Create it (empty) so backend-only builds — CI, fresh clones,
// test runs — work without building the frontend first; the static handler
// serves a "frontend not built" placeholder when it's empty. Vite writes the
// real assets here (see frontend/vite.config.ts).
fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web-dist");
    std::fs::create_dir_all(&dist).expect("create web-dist placeholder");
    // Only rerun for this script itself. In debug builds rust-embed reads the
    // folder live at runtime; in release builds, rebuild after `npm run build`
    // to pick up new assets (cargo cannot track the embed folder).
    println!("cargo:rerun-if-changed=build.rs");
    // Adding a file to migrations/ does not reliably re-expand the
    // `sqlx::migrate!` proc-macro on stable Rust (it served a stale set once,
    // causing "migration N previously applied but missing"). Watching the dir
    // here invalidates the build so a new migration is re-embedded.
    println!("cargo:rerun-if-changed=migrations");

    embed_windows_resources();
}

/// On Windows, embed the app icon + version metadata into the built `.exe`s so
/// pulp.exe/pulpw.exe show the Pulp icon in Explorer, the taskbar, and Alt-Tab
/// and read as a real installed app in file properties. This applies to every
/// binary target in the crate (both `pulp` and `pulpw`). No-op off Windows.
#[cfg(windows)]
fn embed_windows_resources() {
    // wix/pulp.ico is the same icon the MSI uses for the Start Menu shortcut and
    // Add/Remove Programs — reuse it so the exe, shortcut, and ARP all match.
    let icon = "wix/pulp.ico";
    println!("cargo:rerun-if-changed={icon}");

    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon)
        .set("ProductName", "Pulp")
        .set("FileDescription", "Pulp — self-hostable social listening")
        .set("CompanyName", "Nimbus Labs")
        .set("LegalCopyright", "MIT-licensed");
    if let Err(e) = res.compile() {
        // The icon/metadata are cosmetic — a missing or broken resource compiler
        // must not fail the whole build (which also produces the working exe).
        println!("cargo:warning=winresource: could not embed Windows icon/metadata: {e}");
    }
}

#[cfg(not(windows))]
fn embed_windows_resources() {}
