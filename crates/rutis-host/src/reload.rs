//! What makes `run` read its configuration again: SIGUSR1 (Unix), and with
//! `--watch-config` the file's content changing (every platform). Either
//! way the same reload runs ([`reload`]); triggers that come while one runs
//! make one more once it ends.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::time::Instant;

use rutis_host::config::{read_text, HostConfig};
use rutis_host::errln;
use rutis_host::host::Host;

const FLAG: &str = "--watch-config";

/// How often `--watch-config` reads the file.
const POLL: Duration = Duration::from_millis(100);

/// How long the file's new content must stay the same before it is read
/// as the configuration: an editor saving it, or a ConfigMap update, is
/// over by then.
pub const SETTLE: Duration = Duration::from_millis(500);

/// Take `--watch-config` out of `args`: whether it was there.
pub fn watch_flag(args: Vec<String>) -> (bool, Vec<String>) {
    let watch = args.iter().any(|arg| arg == FLAG);
    (watch, args.into_iter().filter(|arg| arg != FLAG).collect())
}

/// What asks for a reload.
pub struct Triggers {
    /// SIGUSR1; `None` on Windows, or when the host was started ignoring it.
    #[cfg(unix)]
    usr1: Option<tokio::signal::unix::Signal>,
    watch: Option<Watch>,
}

impl Triggers {
    /// Listen for SIGUSR1 (Unix), and watch `path` when `watch`; the file's
    /// content now is what the host started with.
    pub fn listen(path: &Path, watch: bool) -> Result<Self, String> {
        Ok(Self {
            #[cfg(unix)]
            usr1: crate::stop::unix_signal(tokio::signal::unix::SignalKind::user_defined1())
                .map_err(|error| format!("cannot listen for SIGUSR1: {error}"))?,
            watch: watch.then(|| Watch::new(path)),
        })
    }

    /// The next trigger: what it was.
    pub async fn next(&mut self) -> &'static str {
        let watched = async {
            match &mut self.watch {
                Some(watch) => watch.changed().await,
                None => std::future::pending().await,
            }
        };
        #[cfg(unix)]
        {
            tokio::select! {
                () = crate::stop::recv(&mut self.usr1) => "SIGUSR1",
                () = watched => "rutis.json changed",
            }
        }
        #[cfg(not(unix))]
        {
            watched.await;
            "rutis.json changed"
        }
    }

    /// The file was read as `text` for a reload: the watch takes it as
    /// seen, so the same content does not trigger again.
    pub fn read(&mut self, text: &str) {
        if let Some(watch) = &mut self.watch {
            watch.seen = Some(text.to_owned());
            watch.settling = None;
        }
    }
}

/// `--watch-config`: the file's content, read every [`POLL`].
struct Watch {
    path: PathBuf,
    /// The content the host runs (or refused), last read.
    seen: Option<String>,
    /// Content other than `seen`, and since when it is the same.
    settling: Option<(String, Instant)>,
}

impl Watch {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            seen: std::fs::read_to_string(path).ok(),
            settling: None,
        }
    }

    /// Wait until the file holds content other than what was seen, the
    /// same for [`SETTLE`]. A file that is not there (an editor replacing
    /// it) is not a change: its next content is.
    async fn changed(&mut self) {
        loop {
            tokio::time::sleep(POLL).await;
            let Ok(now) = std::fs::read_to_string(&self.path) else {
                continue;
            };
            if Some(&now) == self.seen.as_ref() {
                self.settling = None;
                continue;
            }
            match &self.settling {
                Some((settling, since)) if *settling == now => {
                    if since.elapsed() >= SETTLE {
                        self.settling = None;
                        return;
                    }
                }
                _ => self.settling = Some((now, Instant::now())),
            }
        }
    }
}

/// Read `path` again and run it instead ([`Host::reload`]), saying what
/// came of it: `rutis-host: reloaded <path> (<n> rows changed)`, or
/// `rutis-host: reload rejected: <why>; nothing changed`.
pub async fn reload(host: &mut Host, path: &Path, triggers: &mut Triggers) {
    let read = read_text(path).and_then(|text| {
        triggers.read(&text);
        HostConfig::parse(&text, path)
    });
    let config = match read {
        Ok(config) => config,
        Err(error) => {
            errln!("rutis-host: reload rejected: {error}; nothing changed");
            return;
        }
    };
    let warnings = config.warnings.clone();
    match host.reload(config).await {
        Ok(reloaded) => {
            for warning in warnings {
                errln!("rutis-host: warning: {warning}");
            }
            errln!("rutis-host: reloaded {} ({reloaded})", path.display());
            for warning in &reloaded.warnings {
                errln!("rutis-host: warning: {warning}");
            }
        }
        Err(error) => {
            for line in crate::host::rejected_lines(&error) {
                errln!("{line}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_is_taken_out() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert_eq!(
            watch_flag(args(&["--watch-config", "a.json"])),
            (true, args(&["a.json"]))
        );
        assert_eq!(watch_flag(args(&["a.json"])), (false, args(&["a.json"])));
    }

    /// guarantee: host/reload-config
    #[tokio::test(start_paused = true)]
    async fn the_watch_waits_for_new_content_to_settle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rutis.json");
        std::fs::write(&path, "one").unwrap();
        let mut watch = Watch::new(&path);
        // The same content, touched: nothing.
        std::fs::write(&path, "one").unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), watch.changed())
                .await
                .is_err()
        );
        // Gone, then new content (an editor's save): once, after it settled.
        std::fs::remove_file(&path).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        std::fs::write(&path, "two").unwrap();
        let started = Instant::now();
        watch.changed().await;
        assert!(started.elapsed() >= SETTLE, "{:?}", started.elapsed());
        watch.seen = Some("two".into());
        assert!(
            tokio::time::timeout(Duration::from_secs(5), watch.changed())
                .await
                .is_err()
        );
    }

    /// A file that is not there is no change, however long it stays away;
    /// back with the content seen, still none.
    /// guarantee: host/reload-config
    #[tokio::test(start_paused = true)]
    async fn a_missing_file_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rutis.json");
        std::fs::write(&path, "one").unwrap();
        let mut watch = Watch::new(&path);
        std::fs::remove_file(&path).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), watch.changed())
                .await
                .is_err()
        );
        assert_eq!(watch.settling, None);
        std::fs::write(&path, "one").unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), watch.changed())
                .await
                .is_err()
        );
    }
}
