//! `rutis-dsh up`: the dsh web UI in a rutis host, with model calls served by
//! aimux-llm in this process. dsh runs in a Node process that rutis starts,
//! owns and stops; see the crate docs.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("dump-config") {
        std::process::exit(dump_config(&args[1..]));
    }
    run(args);
}

#[cfg(all(unix, dsh_installed))]
#[tokio::main]
async fn run(args: Vec<String>) {
    std::process::exit(host::run(args).await);
}

#[cfg(not(all(unix, dsh_installed)))]
fn run(_args: Vec<String>) {
    eprintln!("rutis-dsh was built without its npm project: run `npm --prefix crates/rutis-dsh/dsh ci` and rebuild (Unix only)");
    std::process::exit(1);
}

const DUMP_USAGE: &str = "\
usage: rutis-dsh dump-config [--profile <name>] [--patch <file>]...

Prints the profile's composed entry list, as rutis-loader sees it, in dsh's
entry-list YAML (`!!js` kept). Compare with `dsh --profile <name> --dump-config`.

  --profile <name>   dsh profile under $DSH_HOME/profiles (default: rutis-web)
  --patch <file>     an overlay applied after the user and home layers";

/// `rutis-dsh dump-config`: compose the profile in Rust and print it.
fn dump_config(args: &[String]) -> i32 {
    use rutis_dsh::profile;
    let mut name = "rutis-web".to_owned();
    let mut overlays = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--profile", Some(value)) => name = value.clone(),
            ("--patch", Some(value)) => overlays.push(value.into()),
            _ => {
                eprintln!("{DUMP_USAGE}");
                return 2;
            }
        }
    }
    let mut context = profile::ProfileContext::named(&name, profile::install_anchor());
    context.overlays = overlays;
    match profile::load(&context) {
        Ok(loaded) => {
            for skipped in &loaded.skipped {
                eprintln!(
                    "rutis-dsh: skipping profile bundle {:?}: {}",
                    skipped.package, skipped.reason
                );
            }
            for issue in &loaded.issues {
                eprintln!("rutis-dsh: {issue}");
            }
            print!("{}", profile::dump(&loaded));
            0
        }
        Err(error) => {
            eprintln!("rutis-dsh: {error}");
            1
        }
    }
}

#[cfg(all(unix, dsh_installed))]
mod host {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use aimux_llm::{AimuxLlmPlugin, LlmService};
    use rutis::host::*;
    use rutis::{Ctx, Error, Plugin};
    use rutis_dsh::events::{Exit, Ready, StartupFailed};
    use rutis_dsh::web;
    use tokio::sync::mpsc;

    const USAGE: &str = "\
usage: rutis-dsh up [--profile <name>] [dsh options...]

Starts the dsh web UI with model calls served by aimux in this process.
Options after `up` other than --profile go to dsh, e.g. --port 3080 --no-open.

  --profile <name>   dsh profile under $DSH_HOME/profiles (default: rutis-web)

The `aimux` model routes are configured on the dsh Models page (settings
section `llm-aimux`). Routes without a key use the fallback model from
AIMUX_PROVIDER / AIMUX_MODEL (default deepseek / deepseek-chat) and that
provider's key variable, e.g. DEEPSEEK_API_KEY.";

    /// Why the host stops, and its exit code.
    enum Stop {
        Interrupted,
        Exit(i32),
        StartupFailed(String),
        ProcessEnded,
    }

    pub async fn run(args: Vec<String>) -> i32 {
        let Some((profile, dsh_args)) = parse(args) else {
            eprintln!("{USAGE}");
            return 2;
        };
        let root = match Ctx::root() {
            Ok(root) => root,
            Err(error) => {
                eprintln!("[rutis-dsh] {error}");
                return 1;
            }
        };
        let code = match serve(&root, profile, dsh_args).await {
            Ok(stop) => match stop {
                Stop::Interrupted => 130,
                Stop::Exit(code) => code,
                Stop::StartupFailed(message) => {
                    eprintln!("[rutis-dsh] dsh failed to start: {message}");
                    1
                }
                Stop::ProcessEnded => {
                    eprintln!("[rutis-dsh] the dsh process ended");
                    1
                }
            },
            Err(error) => {
                eprintln!("[rutis-dsh] {error}");
                1
            }
        };
        if let Err(error) = root.shutdown_with_timeout(Duration::from_secs(10)).await {
            eprintln!("[rutis-dsh] shutdown: {error}");
        }
        code
    }

    fn parse(args: Vec<String>) -> Option<(Option<String>, Vec<String>)> {
        let mut args = args.into_iter();
        if args.next().as_deref() != Some("up") {
            return None;
        }
        let (mut profile, mut rest) = (None, Vec::new());
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--profile" => profile = Some(args.next()?),
                "-h" | "--help" if rest.is_empty() => return None,
                _ => rest.push(arg),
            }
        }
        Some((profile, rest))
    }

    async fn serve(root: &Ctx, profile: Option<String>, args: Vec<String>) -> Result<Stop, Error> {
        let llm = root.plugin(AimuxLlmPlugin::from_env());
        (&llm)
            .await
            .map_err(|error| Error::msg(error.to_string()))?;
        let service: Arc<dyn LlmService> = root
            .get::<dyn LlmService>()
            .ok_or_else(|| Error::from(ErrorKind::ServiceNotFound("aimux-llm".into())))?;
        rutis_dsh::provide_web_aimux(root, service)?;

        let (stop, mut stopped) = mpsc::unbounded_channel();
        root.on::<Ready>(|event, _| async move {
            eprintln!(
                "[rutis-dsh] dsh is up; model routes: {}",
                event.0.join(", ")
            );
            Ok(None)
        })?;
        let forward = stop.clone();
        root.on::<StartupFailed>(move |event, _| {
            let _ = forward.send(Stop::StartupFailed(event.0.clone()));
            async { Ok(None) }
        })?;
        let forward = stop.clone();
        root.on::<Exit>(move |event, _| {
            let _ = forward.send(Stop::Exit(event.0 as i32));
            async { Ok(None) }
        })?;

        let cwd = std::env::current_dir()
            .ok()
            .map(|dir| dir.to_string_lossy().into_owned());
        let view = root.plugin(web::Plugin::new(web::Config {
            profile,
            args: Some(args),
            cwd,
        }));
        (&view)
            .await
            .map_err(|error| Error::msg(error.to_string()))?;
        // Withdrawn when the Node process goes away.
        root.plugin(Watch(Mutex::new(Some(stop))));

        tokio::select! {
            _ = tokio::signal::ctrl_c() => Ok(Stop::Interrupted),
            stop = stopped.recv() => Ok(stop.unwrap_or(Stop::ProcessEnded)),
        }
    }

    /// Depends on the launcher's `rutisDsh` service; its cleanup runs when
    /// the service is withdrawn, i.e. when the dsh process ended.
    struct Watch(Mutex<Option<mpsc::UnboundedSender<Stop>>>);
    impl Plugin for Watch {
        type Deps = Arc<web::Launched>;

        fn name(&self) -> &str {
            "rutis-dsh:watch"
        }

        async fn apply(&self, ctx: &Ctx, _: Arc<web::Launched>) -> rutis::Result {
            let stop = self.0.lock().unwrap().take();
            ctx.effect(|| async move {
                if let Some(stop) = stop {
                    let _ = stop.send(Stop::ProcessEnded);
                }
                Ok(())
            })?;
            Ok(())
        }
    }
}
