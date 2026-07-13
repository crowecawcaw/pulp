//! `pulp serve` — run the server, with agent-friendly process control.
//!
//! `serve` runs one server on a **fixed port** (the configured
//! `server.host:port`, or `BIND`). There is at most one at a time — the port
//! itself is the singleton guard, and the running process records its PID in
//! `<home>/pulp.pid`.
//!
//! Modes:
//! - `pulp serve` — run in the foreground, logging to stdout (and
//!   `server.log`); stops cleanly on Ctrl-C / SIGTERM.
//! - `pulp serve start` — spawn the server in the background and exit 0
//!   once it's accepting connections. Idempotent: already-running is success.
//! - `pulp serve stop` — signal the running server to stop, wait for it,
//!   exit 0. Idempotent: not-running is success.
//! - `pulp serve status` — report whether the server is up (and its PID).
//!
//! Fixed-port policy: if the port is already taken when the foreground server
//! starts, an interactive terminal is *offered* the chance to kill the
//! occupant; a non-interactive run just prints why and exits non-zero.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use clap::Subcommand;

use crate::cli::util;

pub const LONG_ABOUT: &str = "\
Run the Pulp server (HTTP API + embedded web UI + background collectors and
notifier), or control a background instance.

The server runs on a fixed address (server.host:port in ~/.pulp/config.json,
or the BIND env var) and is a singleton — at most one at a time. If the port is
already taken, an interactive terminal is offered the chance to stop the
occupant; a non-interactive run prints why and exits non-zero.

MODES:
  pulp serve            run in the foreground (Ctrl-C / SIGTERM to stop)
  pulp serve start      start in the background, exit 0 once it's accepting
  pulp serve stop       stop the background server, exit 0
  pulp serve status     report whether it's running (and its PID)

`start` and `stop` are idempotent — already-running and not-running are both
success — which makes them safe for agents to call unconditionally.";

/// `pulp serve <cmd>`. Omit `<cmd>` to run in the foreground.
#[derive(Subcommand, Debug)]
pub enum ServeCmd {
    /// Start the server in the background and exit once it's accepting requests
    Start,
    /// Stop the background server and exit
    Stop,
    /// Report whether the server is running (and on which address)
    Status,
}

// ---------------------------------------------------------------------------
// Foreground
// ---------------------------------------------------------------------------

/// Run the server in the foreground until Ctrl-C / SIGTERM. Claims the fixed
/// port (handling an occupied one per the fixed-port policy) and writes/clears
/// the pidfile around the run.
pub async fn foreground() -> anyhow::Result<()> {
    run_server(shutdown_signal()).await
}

/// The reusable server-run wrapper: claim the fixed port, write/clear the
/// pidfile, and serve until the injected `shutdown` future resolves. Both
/// `foreground()` (passing [`shutdown_signal`]) and the desktop-tray launcher
/// (`cli::app`, passing its Quit signal) call this so they share the exact same
/// singleton port-guard + pidfile behavior.
///
/// The port conflict is resolved *interactively* here (a terminal is offered
/// the chance to stop the occupant); GUI callers that must never block on stdin
/// use [`run_server_noninteractive`] instead.
pub async fn run_server(
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    run_server_inner(shutdown, true).await
}

/// Like [`run_server`] but never prompts on stdin when the port is occupied —
/// it fails with a clear error instead. Used by the desktop-tray launcher,
/// where blocking a GUI launch on a stdin prompt would hang.
pub async fn run_server_noninteractive(
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    run_server_inner(shutdown, false).await
}

/// How long to wait for in-flight connections to drain after a shutdown
/// signal before giving up and exiting anyway. `axum::serve`'s graceful
/// shutdown only stops *accepting new* connections on signal — it waits for
/// existing ones to finish on their own, and the feed's SSE stream
/// (`/api/mentions/stream`) is intentionally long-lived (open for as long as
/// a browser tab is), so it never finishes by itself. Without a bound, a
/// single open feed tab would make Ctrl-C / tray-Quit hang forever.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

async fn run_server_inner(
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    interactive: bool,
) -> anyhow::Result<()> {
    let home = crate::config::resolve_home()?;
    crate::config::ensure_home_dirs(&home)?;
    let addr = crate::config::resolve_bind();

    ensure_port_available(&home, &addr, interactive)?;

    let pidfile = pid_path(&home);
    write_pidfile(&pidfile)?;

    // Observe the shutdown signal firing so the grace clock only starts once
    // shutdown is actually *requested* — not from server boot. `server::run`
    // runs for the whole server lifetime, so a plain timeout around it would
    // kill a healthy server after SHUTDOWN_GRACE. Instead we drive the real
    // shutdown future through `observed`, which pings `fired_tx` at the instant
    // it resolves, and only then arm the drain deadline.
    let (fired_tx, fired_rx) = tokio::sync::oneshot::channel::<()>();
    let mut fired_tx = Some(fired_tx);
    let observed = async move {
        shutdown.await;
        if let Some(tx) = fired_tx.take() {
            let _ = tx.send(());
        }
    };
    let drain_deadline = async move {
        // Wait for shutdown to be requested, then bound how long we'll wait for
        // in-flight connections (notably long-lived SSE feeds) to drain.
        let _ = fired_rx.await;
        tokio::time::sleep(SHUTDOWN_GRACE).await;
    };

    let server = crate::server::run(observed);
    tokio::pin!(server);
    let result = tokio::select! {
        result = &mut server => result,
        _ = drain_deadline => {
            tracing::warn!(
                "server did not drain within {:?} (an SSE connection is likely still open); \
                 exiting anyway",
                SHUTDOWN_GRACE
            );
            Ok(())
        }
    };

    // Best-effort: only remove the pidfile if it's still ours, so we don't
    // delete a successor's file on a racy restart.
    if read_pidfile(&pidfile) == Some(std::process::id()) {
        let _ = std::fs::remove_file(&pidfile);
    }
    result
}

/// Resolve the fixed-port conflict before binding. Free → proceed. Occupied →
/// offer to kill the occupant on an interactive terminal, otherwise fail with
/// an actionable message (the agent-friendly non-interactive path).
fn ensure_port_available(home: &Path, addr: &str, interactive: bool) -> anyhow::Result<()> {
    if port_bindable(addr) {
        return Ok(());
    }

    let pid = running_pid(home, addr);
    let who = pid.map(|p| format!(" by pid {}", p)).unwrap_or_default();

    if interactive && std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
        eprint!(
            "Port {} is already in use{}. Stop that process and start here? [y/N] ",
            addr, who
        );
        std::io::stderr().flush().ok();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            bail!("port {} is in use; not starting", addr);
        }
        let pid = pid.with_context(|| {
            format!("could not identify the process holding {} to stop it", addr)
        })?;
        kill_and_wait(pid)?;
        wait_until(Duration::from_secs(5), || port_bindable(addr))
            .with_context(|| format!("port {} did not free up after stopping pid {}", addr, pid))?;
        Ok(())
    } else {
        bail!(
            "port {} is already in use{} — `pulp serve` uses a fixed port. \
             Stop the running server (`pulp serve stop`) or change server.port \
             in {}/config.json.",
            addr,
            who,
            home.display()
        )
    }
}

/// Resolve on Ctrl-C or SIGTERM (so `serve stop` and process managers shut the
/// server down cleanly, letting the foreground path clear its pidfile).
pub(crate) async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    tracing::info!("shutdown signal received; stopping server");
}

// ---------------------------------------------------------------------------
// start / stop / status
// ---------------------------------------------------------------------------

/// Dispatch `serve start|stop|status`. These are short control operations that
/// print to `out` and honor `--json`; they never block serving.
pub fn control(cmd: &ServeCmd, json: bool, out: &mut dyn Write) -> anyhow::Result<()> {
    let home = crate::config::resolve_home()?;
    crate::config::ensure_home_dirs(&home)?;
    let addr = crate::config::resolve_bind();
    match cmd {
        ServeCmd::Start => start(&home, &addr, json, out),
        ServeCmd::Stop => stop(&home, &addr, json, out),
        ServeCmd::Status => status(&home, &addr, json, out),
    }
}

fn start(home: &Path, addr: &str, json: bool, out: &mut dyn Write) -> anyhow::Result<()> {
    // Already up? Treat as success so agents can call `start` unconditionally.
    if port_listening(addr) {
        let pid = running_pid(home, addr);
        return report(out, json, "already-running", pid, addr, "already running");
    }

    let exe = std::env::current_exe().context("locating the pulp executable")?;
    let log = open_log(home)?;
    let child = Command::new(&exe)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(log.try_clone().context("duplicating log handle")?)
        .stderr(log)
        .spawn()
        .with_context(|| format!("spawning background `{} serve`", exe.display()))?;
    let pid = child.id();

    // Exit 0 only once it's actually accepting connections, so a follow-up CLI
    // call won't race the boot.
    if wait_until(Duration::from_secs(15), || port_listening(addr)).is_err() {
        bail!(
            "background server (pid {}) did not start accepting on {} within 15s — \
             check `pulp logs`",
            pid,
            addr
        );
    }
    report(out, json, "started", Some(pid), addr, "started")
}

fn stop(home: &Path, addr: &str, json: bool, out: &mut dyn Write) -> anyhow::Result<()> {
    let pidfile = pid_path(home);
    // Resolve "what's running" the same way `status` does (pidfile, else
    // whatever `lsof` reports holding the port) so the two commands can never
    // disagree — `status` used to also credit a foreign process holding the
    // port via the lsof fallback, while `stop` only ever looked at the
    // pidfile, so a stale/missing pidfile made `status` say "running" and
    // `stop` say "not running" in the same breath, leaving nothing to signal.
    //
    // Before signalling, additionally verify the PID is still actually our
    // own executable (`pid_matches_pulp`): the pidfile only records a bare
    // number, and if that process exited and the OS recycled the PID to an
    // unrelated program, blindly `kill`ing it would hit the wrong process.
    match running_pid(home, addr).filter(|&p| pid_matches_pulp(p)) {
        Some(pid) => {
            kill_and_wait(pid)?;
            let _ = std::fs::remove_file(&pidfile);
            report(out, json, "stopped", Some(pid), addr, "stopped")
        }
        None => {
            // Clear a stale pidfile so status stays honest.
            let _ = std::fs::remove_file(&pidfile);
            report(out, json, "not-running", None, addr, "not running")
        }
    }
}

fn status(home: &Path, addr: &str, json: bool, out: &mut dyn Write) -> anyhow::Result<()> {
    let pid = running_pid(home, addr);
    let listening = port_listening(addr);
    let running = pid.is_some() || listening;
    if json {
        util::print_json(
            out,
            &serde_json::json!({
                "running": running,
                "pid": pid,
                "address": addr,
                "listening": listening,
            }),
        )?;
    } else if running {
        match pid {
            Some(pid) => writeln!(out, "running (pid {}) on http://{}", pid, addr)?,
            None => writeln!(out, "running on http://{} (pid unknown)", addr)?,
        }
    } else {
        writeln!(out, "not running (would serve on http://{})", addr)?;
    }
    Ok(())
}

/// Uniform success line / JSON object for the control commands.
fn report(
    out: &mut dyn Write,
    json: bool,
    state: &str,
    pid: Option<u32>,
    addr: &str,
    human: &str,
) -> anyhow::Result<()> {
    if json {
        util::print_json(
            out,
            &serde_json::json!({ "status": state, "pid": pid, "address": addr }),
        )?;
    } else {
        match pid {
            Some(pid) => writeln!(out, "{} (pid {}) on http://{}", human, pid, addr)?,
            None => writeln!(out, "{} (http://{})", human, addr)?,
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// pidfile + process + port helpers
// ---------------------------------------------------------------------------

fn pid_path(home: &Path) -> PathBuf {
    home.join("pulp.pid")
}

fn write_pidfile(pidfile: &Path) -> anyhow::Result<()> {
    std::fs::write(pidfile, std::process::id().to_string())
        .with_context(|| format!("writing pidfile {}", pidfile.display()))
}

fn read_pidfile(pidfile: &Path) -> Option<u32> {
    std::fs::read_to_string(pidfile).ok()?.trim().parse().ok()
}

/// The PID of the running server: a live pidfile entry, else (for a server
/// started outside this app home) whatever lsof reports holding the port.
fn running_pid(home: &Path, addr: &str) -> Option<u32> {
    read_pidfile(&pid_path(home))
        .filter(|p| pid_alive(*p))
        .or_else(|| pid_on_port(addr))
}

/// Best-effort check that `pid` is actually running OUR executable before
/// `stop` signals it. The pidfile (or the lsof-on-port fallback) only gives a
/// bare PID number — if the process it named has since exited and the OS
/// recycled that PID to an unrelated program, blindly `kill`ing it would hit
/// the wrong process. When identity can't be determined at all (no
/// `current_exe`, or the platform tool is missing), fail open (`true`) rather
/// than make `stop` permanently unable to stop anything.
fn pid_matches_pulp(pid: u32) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return true;
    };
    match exe.file_name().and_then(|n| n.to_str()) {
        Some(expected) => process_name_matches(pid, expected),
        None => true,
    }
}

/// True if the OS reports `pid`'s process name as `expected` (or a
/// truncation of it — some platforms' `comm`/image-name fields cap process
/// name length). Shells out to `ps`/`tasklist` rather than reading procfs
/// directly, matching this module's existing no-native-dependency style
/// ([`pid_alive`], [`kill_and_wait`]). Returns `false` if the tool reports no
/// such process (already exited) or is unavailable.
fn process_name_matches(pid: u32, expected: &str) -> bool {
    #[cfg(unix)]
    {
        let output = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output();
        match output {
            Ok(out) if out.status.success() => {
                let name = String::from_utf8_lossy(&out.stdout);
                let name = name.trim();
                if name.is_empty() {
                    return false;
                }
                // macOS `ps -o comm=` prints the full executable path
                // (`/…/target/debug/pulp`); Linux prints the basename (truncated
                // to 15 chars). Reduce to a basename before comparing so the path
                // form still matches, and keep the prefix checks to tolerate the
                // Linux length cap.
                let name = std::path::Path::new(name)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(name);
                name == expected || expected.starts_with(name) || name.starts_with(expected)
            }
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        // Reuse the same CSV row shape `pid_alive` parses: the first field is
        // the quoted image name, e.g. `"pulp.exe","12345",...`.
        let output = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH", "/FO", "CSV"])
            .stderr(Stdio::null())
            .output();
        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                text.lines()
                    .filter(|l| l.starts_with('"') && l.contains(&format!("\"{}\"", pid)))
                    .filter_map(|l| l.split(',').next())
                    .map(|f| f.trim_matches('"'))
                    .any(|name| {
                        name.eq_ignore_ascii_case(expected)
                            || expected
                                .eq_ignore_ascii_case(name.strip_suffix(".exe").unwrap_or(name))
                    })
            }
            _ => false,
        }
    }
}

/// True if `addr` can be bound right now (i.e. nothing is listening). Binds and
/// immediately drops — a tiny TOCTOU window that's fine for a singleton CLI.
fn port_bindable(addr: &str) -> bool {
    std::net::TcpListener::bind(addr).is_ok()
}

/// True if something is accepting connections on `addr`. `0.0.0.0` isn't a
/// connectable address, so probe loopback in that case.
fn port_listening(addr: &str) -> bool {
    std::net::TcpStream::connect(connect_addr(addr)).is_ok()
}

/// Rewrite a bind address into one a local client can actually connect to.
/// Wildcard hosts (`0.0.0.0`, `::`, or an empty host) aren't connectable, so
/// they map to loopback; everything else is returned unchanged. Shared with
/// `tls::install_url` and the desktop-tray launcher's "Open Pulp" URL.
pub(crate) fn connect_addr(addr: &str) -> String {
    match addr.rsplit_once(':') {
        Some((host, port)) if host == "0.0.0.0" || host == "::" || host.is_empty() => {
            format!("127.0.0.1:{}", port)
        }
        _ => addr.to_string(),
    }
}

/// Best-effort lookup of the PID listening on `addr`'s port via `lsof` (for a
/// server not started through this app home, so with no pidfile). `None` if
/// `lsof` is missing or finds nothing.
fn pid_on_port(addr: &str) -> Option<u32> {
    let port = addr.rsplit(':').next()?;
    let output = Command::new("lsof")
        .args(["-ti", &format!("tcp:{}", port), "-sTCP:LISTEN"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Liveness check — true if the process exists and is signalable. Shells out to
/// the platform's built-in tool (no libc/nix/winapi native dependency, mirroring
/// the deliberate dependency-free stance) — `kill -0` on Unix, `tasklist` on
/// Windows.
fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        signal(pid, "0")
    }
    #[cfg(windows)]
    {
        // `tasklist /FI "PID eq <pid>"` prints a process row when the PID exists
        // and an informational "No tasks..." line (to stdout, exit 0) when it
        // doesn't — so we can't rely on the exit status. Instead, confirm the
        // PID actually appears in a process row of the CSV output.
        let output = Command::new("tasklist")
            .args([
                "/FI",
                &format!("PID eq {}", pid),
                "/NH", // no header row
                "/FO",
                "CSV",
            ])
            .stderr(Stdio::null())
            .output();
        match output {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout);
                // Each matching process is a CSV row whose 2nd field is the PID,
                // e.g. `"pulp.exe","12345","Console","1","42,000 K"`. The
                // no-match case is a plain `INFO:`-prefixed sentence, which won't
                // contain a `"<pid>"` field.
                let needle = format!("\"{}\"", pid);
                text.lines()
                    .any(|line| line.starts_with('"') && line.contains(&needle))
            }
            _ => false,
        }
    }
}

/// Send signal `sig` to `pid` via the `kill` binary (avoids a libc/nix native
/// dependency for what is a rare, terminal control action).
#[cfg(unix)]
fn signal(pid: u32, sig: &str) -> bool {
    Command::new("kill")
        .arg(format!("-{}", sig))
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Ask `pid` to stop, wait for graceful exit, then force-kill if it lingers.
/// On Unix that's SIGTERM then SIGKILL (`kill`); on Windows it's
/// `taskkill /T` then `taskkill /T /F` (no native winapi dependency).
fn kill_and_wait(pid: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        signal(pid, "TERM");
        if wait_until(Duration::from_secs(10), || !pid_alive(pid)).is_ok() {
            return Ok(());
        }
        signal(pid, "KILL");
        wait_until(Duration::from_secs(3), || !pid_alive(pid))
            .with_context(|| format!("process {} did not exit after SIGTERM/SIGKILL", pid))
    }
    #[cfg(windows)]
    {
        // Graceful first: `/T` ends the whole process tree (the background
        // `pulp serve` plus anything it spawned).
        taskkill(pid, false);
        if wait_until(Duration::from_secs(10), || !pid_alive(pid)).is_ok() {
            return Ok(());
        }
        // Force the kill if it lingered. (Windows has no SIGTERM-style graceful
        // signal for a non-console child, so `taskkill` without `/F` may not
        // stop a service-like process at all; `/F` is the reliable stop.)
        taskkill(pid, true);
        wait_until(Duration::from_secs(3), || !pid_alive(pid))
            .with_context(|| format!("process {} did not exit after taskkill /T [/F]", pid))
    }
}

/// Terminate `pid` (and its tree) with `taskkill`. `force` adds `/F`.
#[cfg(windows)]
fn taskkill(pid: u32, force: bool) -> bool {
    let mut cmd = Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T"]);
    if force {
        cmd.arg("/F");
    }
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Poll `cond` every 100 ms until it's true or `timeout` elapses.
fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("condition not met within {:?}", timeout);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn open_log(home: &Path) -> anyhow::Result<std::fs::File> {
    let path = crate::cli::server_log_path().unwrap_or_else(|_| home.join("server.log"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening log file {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_addr_redirects_wildcard_hosts_to_loopback() {
        assert_eq!(connect_addr("0.0.0.0:3000"), "127.0.0.1:3000");
        assert_eq!(connect_addr(":::3000"), "127.0.0.1:3000");
        assert_eq!(connect_addr("127.0.0.1:3000"), "127.0.0.1:3000");
        assert_eq!(connect_addr("example.com:8080"), "example.com:8080");
    }

    #[cfg(unix)]
    #[test]
    fn process_name_matches_identifies_and_rejects_processes() {
        // A spawned `sleep` child really is named "sleep" — the positive
        // case `stop` relies on when the pidfile's PID really is our server.
        let mut child = Command::new("sleep").arg("2").spawn().unwrap();
        let pid = child.id();
        assert!(process_name_matches(pid, "sleep"));
        // An arbitrary unrelated expected name must not match — this is the
        // safety net that stops `stop` from signalling a PID the OS recycled
        // to some other program after our server exited.
        assert!(!process_name_matches(pid, "definitely-not-pulp"));
        child.kill().ok();
        child.wait().ok();
    }

    #[test]
    fn process_name_matches_false_for_a_dead_pid() {
        // No process at all → never treated as a match (nothing to signal).
        assert!(!process_name_matches(2_000_000_000, "anything"));
    }

    #[test]
    fn pidfile_round_trips_and_tolerates_garbage() {
        let dir = std::env::temp_dir().join(format!("pulp-serve-test-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let pidfile = pid_path(&dir);

        assert_eq!(read_pidfile(&pidfile), None);
        write_pidfile(&pidfile).unwrap();
        assert_eq!(read_pidfile(&pidfile), Some(std::process::id()));

        std::fs::write(&pidfile, "not-a-pid").unwrap();
        assert_eq!(read_pidfile(&pidfile), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_held_port_is_not_bindable_but_is_connectable() {
        // A listening socket can be connected to but not re-bound; that's the
        // distinction `start`/`status` rely on. (We don't assert on the
        // released port — the OS may briefly hold it in TIME_WAIT.)
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        assert!(!port_bindable(&addr), "held port must not be bindable");
        assert!(port_listening(&addr), "held port must accept connections");
    }

    #[test]
    fn pid_alive_is_true_for_self_false_for_unused_high_pid() {
        assert!(pid_alive(std::process::id()));
        // PIDs are capped well below this; nothing should be alive here.
        assert!(!pid_alive(2_000_000_000));
    }

    #[test]
    fn pid_alive_tracks_a_child_across_its_lifetime() {
        // A spawned child is alive while running and not after it exits + is
        // reaped. We sleep ~2s in the child so the liveness probe (which itself
        // spawns `tasklist`/`kill`) has comfortable headroom.
        #[cfg(unix)]
        let mut child = Command::new("sleep")
            .arg("2")
            .spawn()
            .expect("spawn sleep child");
        #[cfg(windows)]
        let mut child = {
            // `timeout` needs a console; `ping` is the standard scriptless sleep.
            // `-n 3` ≈ 2s. Redirect output so it doesn't clutter test logs.
            Command::new("ping")
                .args(["-n", "3", "127.0.0.1"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn ping child")
        };

        let pid = child.id();
        assert!(pid_alive(pid), "freshly spawned child must be alive");

        child.wait().expect("await child exit");
        // After wait() the child is reaped; on both platforms the PID no longer
        // resolves to a live process. (A tiny chance the OS recycles the PID to
        // an unrelated process exists, but is negligible in a test run.)
        assert!(
            wait_until(Duration::from_secs(3), || !pid_alive(pid)).is_ok(),
            "child must not be alive after it exits and is reaped"
        );
    }
}
