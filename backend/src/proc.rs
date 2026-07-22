//! Spawning child processes cleanly on the Windows desktop app.
//!
//! `pulpw.exe` is a windowless GUI process (no console of its own). When such a
//! process spawns a **console** child — `tailscale`, `tasklist`, `taskkill`, … —
//! Windows allocates a brand-new console window for that child, which flashes on
//! screen. Routing every child spawn through [`command`] sets `CREATE_NO_WINDOW`
//! so no console ever appears. It is a no-op off Windows.

use std::ffi::OsStr;
use std::process::Command;

/// Like [`Command::new`], but on Windows the child is created with
/// `CREATE_NO_WINDOW` so it never pops a console window. No-op elsewhere.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW — run the console child with no visible window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
