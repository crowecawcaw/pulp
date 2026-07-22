//! `pulpw` — the windowless desktop launcher for Pulp on Windows.
//!
//! A single `.exe` on Windows carries exactly one subsystem in its PE header:
//! `console` (a terminal is allocated/attached — right for a CLI) or `windows`
//! (no console is ever created — right for a GUI/tray app). You cannot have
//! both in one binary, so Pulp ships two, mirroring Python's
//! `python.exe`/`pythonw.exe`:
//!
//! - **`pulp.exe`** — console subsystem: the CLI (`pulp serve`, `pulp monitors`,
//!   …). Runs in a terminal with normal stdout/stderr. Unchanged.
//! - **`pulpw.exe`** (this binary) — `windows` subsystem: the desktop app. No
//!   console window ever appears. It runs the system-tray launcher (`pulp app`),
//!   so the Start Menu shortcut and the run-at-login item launch straight into
//!   the tray with nothing but the taskbar/notification-area icon.
//!
//! The `windows_subsystem` attribute is a no-op on non-Windows targets, so this
//! binary still compiles elsewhere (it just isn't shipped there — macOS uses the
//! `.app` bundle around `pulp`, Linux uses `pulp app` directly).
//!
//! This binary only exists in builds with `--features tray` (see the
//! `required-features` on its `[[bin]]` target in Cargo.toml); the CLI dispatch
//! and the tray event loop live in the `pulp` library, so this file is a thin
//! shell that just defaults a no-argument launch to `app`.
#![cfg_attr(windows, windows_subsystem = "windows")]

use clap::Parser;

fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    // Launched with no subcommand — from the Start Menu shortcut, the
    // run-at-login item, or a double-click in Explorer — default to `app` (the
    // tray launcher). Bare `pulp` prints help; bare `pulpw` starts the app.
    // `imply_app_subcommand` (in the library, unit-tested there) inserts `app`
    // for a bare or flags-only launch and passes an explicit subcommand through.
    let raw: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let cli = pulp::cli::Cli::parse_from(pulp::cli::imply_app_subcommand(&raw));

    // The tray event loop (`tao`) must own the real main thread, and it never
    // returns — so, exactly as in `pulp.exe`'s `main`, hand off to the blocking
    // tray entrypoint BEFORE any tokio runtime is built. `run_blocking` spawns
    // the server on its own thread (with its own runtime) and runs the GUI loop
    // here. `--no-tray` falls through to the headless foreground server.
    #[cfg(feature = "tray")]
    if let pulp::cli::Command::App(args) = &cli.command {
        if !args.no_tray {
            return pulp::cli::app::run_blocking(args.clone());
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(pulp::cli::run(cli))
}
