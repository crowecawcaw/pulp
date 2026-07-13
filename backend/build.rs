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
}
