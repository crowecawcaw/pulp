//! `pulp app` — the desktop launcher (system tray / menubar) for
//! non-technical users.
//!
//! `pulp app` starts the Pulp server **in-process** and shows a tray icon that
//! lives exactly as long as the server runs. The tray gives: open the web UI
//! locally, see the URL to open on a phone (to install the PWA over Tailscale),
//! toggle start-at-login, open the logs folder, and quit (which stops the
//! server and exits).
//!
//! ## Build shape
//!
//! ALL tray functionality lives behind the off-by-default `tray` Cargo feature
//! (it pulls GTK3 + libayatana-appindicator on Linux). The `App` subcommand
//! always exists in the CLI so the help text is stable, but:
//!
//! - built WITH `--features tray`: [`run_blocking`] runs the GUI event loop on
//!   the real main thread and the server on a separate thread (see below).
//! - built WITHOUT the feature: `pulp app` prints a one-line notice and then
//!   runs the server in the foreground anyway (so it still works headless).
//!   That path lives in `cli::run` (it needs the async runtime), not here.
//!
//! ## Threading (tray build)
//!
//! The GUI event loop (`tao`) must own the real main OS thread (a macOS
//! requirement) and `EventLoop::run` never returns. So `main` calls
//! [`run_blocking`] BEFORE any tokio runtime is built: it spawns the server on
//! its own thread (with its own tokio runtime) and then runs the event loop on
//! the main thread. Quit fires a oneshot that resolves the server's shutdown
//! future; we join the server thread (so it clears its pidfile) before exiting.

use clap::Args;

/// `pulp app [--no-autostart] [--no-tray]`.
#[derive(Args, Debug, Clone)]
pub struct AppArgs {
    /// Do NOT register/enable the run-at-login item (the permission-sensitive
    /// behavior). By default the launcher enables start-at-login on first run.
    #[arg(long)]
    pub no_autostart: bool,

    /// Run the server in the foreground with no tray icon (for headless/CLI
    /// users or systems with no GUI session). Equivalent to `pulp serve` under
    /// the `app` name.
    #[arg(long)]
    pub no_tray: bool,
}

pub const LONG_ABOUT: &str = "\
Desktop launcher for Pulp — a system-tray / menubar icon for non-technical
users.

`pulp app` starts the Pulp server in-process and shows a tray icon that lives
exactly as long as the server runs. From the tray you can open the web UI
locally, see the URL to open on your phone (to install the PWA over Tailscale),
toggle start-at-login, open the logs folder, and Quit.

The server keeps running in the background for as long as the tray icon is
present — so a phone on the same tailnet can reach the installed PWA — until you
choose Quit from the tray, which stops the server and exits.

FLAGS:
  --no-autostart   don't enable run-at-login (enabled by default on first run)
  --no-tray        run headless in the foreground with no tray icon

Tray support is compiled in only when the binary is built with
`--features tray` (it needs GTK3 + libayatana-appindicator on Linux). A build
without it falls back to running the server in the foreground.";

// ---------------------------------------------------------------------------
// Pure helpers (always compiled, so they are unit-testable without a display).
// ---------------------------------------------------------------------------

/// The disabled header line at the top of the tray menu.
// Consumed by the tray build (`imp`) and by the unit tests; unused in a plain
// default (no-tray, no-test) build.
#[cfg_attr(not(feature = "tray"), allow(dead_code))]
fn status_label(bind: &str) -> String {
    format!("Pulp — running on {}", bind)
}

/// The disabled, human-readable "type this on your phone" label.
#[cfg_attr(not(feature = "tray"), allow(dead_code))]
fn phone_label(install_url: &str) -> String {
    format!("On your phone: {}", install_url)
}

// ---------------------------------------------------------------------------
// Tray implementation (only compiled with `--features tray`).
// ---------------------------------------------------------------------------

#[cfg(feature = "tray")]
pub use imp::run_blocking;

#[cfg(feature = "tray")]
mod imp {
    use super::{phone_label, status_label, AppArgs};

    use anyhow::Context;
    use tao::event::{Event, StartCause};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    // `tray-icon` re-exports its matching `muda` as `tray_icon::menu`, so we do
    // not depend on `muda` directly (keeps the versions in lockstep).
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    /// User events forwarded from tray-icon's global channels onto the tao
    /// event loop (which owns the main thread).
    enum UserEvent {
        Menu(MenuEvent),
    }

    /// Everything the running tray owns. Kept alive for the whole session (the
    /// menu items must outlive the menu, and we match click events by their
    /// ids). Built lazily on `StartCause::Init` because on Linux the tray must
    /// be created after the event loop has initialised GTK.
    struct AppTray {
        // The tray icon handle — dropping it removes the icon, so keep it.
        _tray: TrayIcon,
        open_url: String,
        install_url: String,
        logs_dir: std::path::PathBuf,
        open_id: tray_icon::menu::MenuId,
        open_phone_id: tray_icon::menu::MenuId,
        start_login: CheckMenuItem,
        logs_id: tray_icon::menu::MenuId,
        quit_id: tray_icon::menu::MenuId,
    }

    /// Blocking entrypoint for the tray path (called from `main` on the real
    /// main thread). Spawns the server thread, then runs the GUI event loop
    /// here. Never returns — the event loop diverges and exits the process on
    /// Quit.
    pub fn run_blocking(args: AppArgs) -> anyhow::Result<()> {
        // Route logging to `<home>/server.log` before anything starts. The tray
        // launcher (especially `pulpw.exe`, a windowless GUI process) has no
        // console, so without this the server's tracing output would be dropped
        // and the tray's "Open logs folder" would show nothing.
        crate::cli::init_serve_logging();

        // Resolve the URLs once, up front, off the GUI event loop. `install_url`
        // shells out to `tailscale` (blocking, with a cert side effect), which
        // must never run on the event-loop thread — doing it here, before the
        // loop starts, is safe and caches the result for the static menu.
        let config = crate::config::Config::load()?;
        let bind = config.bind.clone();
        let open_url = format!("http://{}", crate::cli::serve::connect_addr(&bind));
        let install_url = crate::tls::install_url(&config);
        let logs_dir = crate::cli::server_log_path()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| config.home.clone());

        // Determine the start-at-login state up front for the initial menu
        // check mark, enabling it on first run unless opted out.
        let autostart_enabled = init_autostart(&args);

        // --- Server thread ---------------------------------------------------
        // Its own tokio runtime; serves until `shutdown_rx` resolves. A oneshot
        // is race-free (unlike a bare Notify): the permit persists even if the
        // send happens before the receiver is awaited.
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server_thread = std::thread::Builder::new()
            .name("pulp-server".into())
            .spawn(move || -> anyhow::Result<()> {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .context("building server runtime")?;
                rt.block_on(crate::cli::serve::run_server_noninteractive(async move {
                    let _ = shutdown_rx.await;
                }))
            })
            .context("spawning server thread")?;

        // --- GUI event loop (owns the main thread) ---------------------------
        let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

        // Forward tray-icon's global menu-event channel onto our event loop so
        // clicks wake the (otherwise idle) loop.
        let proxy = event_loop.create_proxy();
        MenuEvent::set_event_handler(Some(move |event| {
            let _ = proxy.send_event(UserEvent::Menu(event));
        }));

        let mut app_tray: Option<AppTray> = None;
        // Moved into the closure; taken and joined on Quit so the server clears
        // its pidfile before we exit.
        let mut server_thread = Some(server_thread);
        let mut shutdown_tx = Some(shutdown_tx);

        event_loop.run(move |event, _target, control_flow| {
            // Idle between events; the forwarded menu events wake us.
            *control_flow = ControlFlow::Wait;

            match event {
                Event::NewEvents(StartCause::Init) => {
                    match build_tray(&open_url, &install_url, &logs_dir, autostart_enabled) {
                        Ok(tray) => app_tray = Some(tray),
                        Err(e) => {
                            eprintln!("failed to create tray icon: {e}");
                            // No tray means no way to Quit — shut down and exit.
                            if let Some(tx) = shutdown_tx.take() {
                                let _ = tx.send(());
                            }
                            if let Some(h) = server_thread.take() {
                                let _ = h.join();
                            }
                            *control_flow = ControlFlow::Exit;
                        }
                    }
                }
                Event::UserEvent(UserEvent::Menu(menu_event)) => {
                    let Some(tray) = app_tray.as_ref() else {
                        return;
                    };
                    let id = menu_event.id;
                    if id == tray.open_id {
                        if let Err(e) = open::that_detached(&tray.open_url) {
                            eprintln!("could not open {}: {e}", tray.open_url);
                        }
                    } else if id == tray.open_phone_id {
                        if let Err(e) = open::that_detached(&tray.install_url) {
                            eprintln!("could not open {}: {e}", tray.install_url);
                        }
                    } else if id == *tray.start_login.id() {
                        // muda has already flipped the check state on click;
                        // sync the login item to match.
                        let want = tray.start_login.is_checked();
                        set_autostart(want);
                    } else if id == tray.logs_id {
                        if let Err(e) = open::that_detached(&tray.logs_dir) {
                            eprintln!("could not open {}: {e}", tray.logs_dir.display());
                        }
                    } else if id == tray.quit_id {
                        // Fire shutdown, wait for the server to unwind (so it
                        // removes its pidfile), then exit the loop/process.
                        if let Some(tx) = shutdown_tx.take() {
                            let _ = tx.send(());
                        }
                        if let Some(h) = server_thread.take() {
                            let _ = h.join();
                        }
                        *control_flow = ControlFlow::Exit;
                    }
                }
                _ => {}
            }
        });
    }

    /// Build the tray icon and its menu. Must be called after the event loop
    /// has started (GTK init) on Linux.
    fn build_tray(
        open_url: &str,
        install_url: &str,
        logs_dir: &std::path::Path,
        autostart_enabled: bool,
    ) -> anyhow::Result<AppTray> {
        let bind = open_url.trim_start_matches("http://");

        let menu = Menu::new();
        // Disabled header + phone label (informational only).
        let header = MenuItem::new(status_label(bind), false, None);
        let open = MenuItem::new("Open Pulp", true, None);
        let phone = MenuItem::new(phone_label(install_url), false, None);
        // future: a clipboard-copy item and/or a QR code for the phone URL.
        let open_phone = MenuItem::new("Open on this network (for phone)", true, None);
        let start_login = CheckMenuItem::new("Start at login", true, autostart_enabled, None);
        let logs = MenuItem::new("Open logs folder", true, None);
        let quit = MenuItem::new("Quit Pulp", true, None);

        menu.append_items(&[
            &header,
            &open,
            &phone,
            &open_phone,
            &PredefinedMenuItem::separator(),
            &start_login,
            &logs,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .context("building tray menu")?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Pulp is running")
            .with_icon(load_icon()?)
            .build()
            .context("building tray icon")?;

        Ok(AppTray {
            _tray: tray,
            open_url: open_url.to_string(),
            install_url: install_url.to_string(),
            logs_dir: logs_dir.to_path_buf(),
            open_id: open.id().clone(),
            open_phone_id: open_phone.id().clone(),
            start_login,
            logs_id: logs.id().clone(),
            quit_id: quit.id().clone(),
        })
    }

    /// Decode the embedded brand PNG and downscale it to a tray-sized RGBA icon.
    fn load_icon() -> anyhow::Result<Icon> {
        // Path is relative to THIS source file: backend/src/cli/app.rs ->
        // ../../../frontend/public/pwa-512x512.png.
        const PNG: &[u8] = include_bytes!("../../../frontend/public/pwa-512x512.png");
        let img = image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
            .context("decoding embedded tray icon")?
            .resize_exact(32, 32, image::imageops::FilterType::Triangle)
            .into_rgba8();
        let (w, h) = img.dimensions();
        Icon::from_rgba(img.into_raw(), w, h).context("building tray icon from rgba")
    }

    /// Build the `auto-launch` handle for the login item: relaunch `pulp app`
    /// at login using this binary's absolute path.
    fn autolaunch() -> anyhow::Result<auto_launch::AutoLaunch> {
        let exe = std::env::current_exe().context("locating the pulp executable")?;
        let app_path = exe.to_string_lossy().to_string();
        let mut builder = auto_launch::AutoLaunchBuilder::new();
        builder
            .set_app_name("Pulp")
            .set_app_path(&app_path)
            .set_args(&["app"]);
        #[cfg(target_os = "macos")]
        builder.set_use_launch_agent(true);
        builder.build().context("configuring start-at-login")
    }

    /// Resolve the initial start-at-login state, enabling it on first run
    /// unless `--no-autostart` was given. Best-effort: failures just leave it
    /// disabled rather than aborting the launch.
    fn init_autostart(args: &AppArgs) -> bool {
        let Ok(al) = autolaunch() else {
            return false;
        };
        let enabled = al.is_enabled().unwrap_or(false);
        if !args.no_autostart && !enabled {
            if al.enable().is_ok() {
                return true;
            }
        }
        enabled
    }

    /// Enable or disable the login item to match the tray checkbox.
    fn set_autostart(enable: bool) {
        let Ok(al) = autolaunch() else {
            eprintln!("could not configure start-at-login");
            return;
        };
        let result = if enable { al.enable() } else { al.disable() };
        if let Err(e) = result {
            eprintln!("could not update start-at-login: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_label_reads_naturally() {
        assert_eq!(
            status_label("127.0.0.1:3000"),
            "Pulp — running on 127.0.0.1:3000"
        );
    }

    #[test]
    fn phone_label_includes_the_url() {
        assert_eq!(
            phone_label("https://box.tailnet.ts.net:3443"),
            "On your phone: https://box.tailnet.ts.net:3443"
        );
    }
}
