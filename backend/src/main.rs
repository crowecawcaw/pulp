use clap::Parser;

fn main() -> anyhow::Result<()> {
    // `pulp --dump-openapi` prints the OpenAPI spec to stdout and exits.
    // Kept (alongside `pulp openapi`) because the documented regen
    // workflow is `cargo run -- --dump-openapi > openapi.json`.
    if std::env::args().any(|a| a == "--dump-openapi") {
        use utoipa::OpenApi;
        println!("{}", pulp::api::ApiDoc::openapi().to_pretty_json()?);
        return Ok(());
    }

    dotenvy::dotenv().ok();

    // When double-clicked in Finder, a macOS `.app` runs its bundled executable
    // with no subcommand — but bare `pulp` prints help and exits. Detect the
    // bundle launch (our binary living at `…/Pulp.app/Contents/MacOS/pulp`, with
    // no real args) and default it to `app` so the desktop launcher starts. We
    // filter a legacy `-psn_…` process-serial argument some Finder launches add.
    let cli = if launched_from_app_bundle() {
        pulp::cli::Cli::parse_from(["pulp", "app"])
    } else {
        pulp::cli::Cli::parse()
    };

    // The desktop-tray launcher (`pulp app`) must run its GUI event loop on the
    // real main thread (a macOS requirement) and that loop never returns, so it
    // cannot share a thread with an async runtime. When this binary is built
    // with the `tray` feature AND `--no-tray` was not passed, hand off to the
    // blocking tray entrypoint BEFORE any tokio runtime is built: it spawns the
    // server on its own thread (with its own runtime) and runs the event loop
    // here. Every other command (and the headless `app` fallbacks) build a
    // runtime and go through `cli::run` exactly as before.
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

/// True when this process is the executable inside a macOS `.app` bundle and
/// was started with no subcommand (a Finder/`open` launch) — the cue to default
/// to `pulp app`. Off-macOS, or when run with real arguments, returns false so
/// the normal CLI (including bare-`pulp` help) is untouched.
fn launched_from_app_bundle() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let exe = std::env::current_exe().ok();
    let exe = exe.as_deref().map(|p| p.to_string_lossy());
    let args = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned());
    is_bundle_launch(exe.as_deref(), args)
}

/// Pure decision behind [`launched_from_app_bundle`], split out so it can be
/// unit-tested without touching the real process env: true iff `exe_path` lives
/// inside a `.app/Contents/MacOS/` and `args` (argv without argv[0]) carries no
/// real argument — a lone `-psn_…` process-serial token from Finder doesn't count.
fn is_bundle_launch(exe_path: Option<&str>, args: impl IntoIterator<Item = String>) -> bool {
    let has_real_args = args.into_iter().any(|a| !a.starts_with("-psn_"));
    if has_real_args {
        return false;
    }
    exe_path
        .map(|p| p.contains(".app/Contents/MacOS/"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::is_bundle_launch;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    const IN_BUNDLE: &str = "/Applications/Pulp.app/Contents/MacOS/pulp";
    const OUT_OF_BUNDLE: &str = "/usr/local/bin/pulp";

    #[test]
    fn bundle_launch_no_args() {
        assert!(is_bundle_launch(Some(IN_BUNDLE), args(&[])));
    }

    #[test]
    fn bundle_launch_ignores_psn_token() {
        assert!(is_bundle_launch(Some(IN_BUNDLE), args(&["-psn_0_12345"])));
    }

    #[test]
    fn bundle_with_real_subcommand_is_normal_cli() {
        assert!(!is_bundle_launch(Some(IN_BUNDLE), args(&["serve"])));
        assert!(!is_bundle_launch(
            Some(IN_BUNDLE),
            args(&["-psn_0_1", "serve"])
        ));
    }

    #[test]
    fn outside_bundle_is_never_a_bundle_launch() {
        assert!(!is_bundle_launch(Some(OUT_OF_BUNDLE), args(&[])));
    }

    #[test]
    fn missing_exe_path_is_not_a_bundle_launch() {
        assert!(!is_bundle_launch(None, args(&[])));
    }
}
