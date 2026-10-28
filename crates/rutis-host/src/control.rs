//! `rutis-host status`: a host started by `run` or `dev` answers it on a
//! local socket that only its user can open (design §5.3). The socket
//! answers `status` with [`Loader::status`] as printed, which changes
//! nothing; and `call` (`rutis-host call`, try-and-test design §4), which
//! `dev` accepts and `run` only with `--allow-call`. The format on the
//! socket is rutis-host's own: both ends are the same rutis-host. It sits in
//! the system's runtime directory
//! (`$XDG_RUNTIME_DIR`, else the temporary directory), in a directory of
//! the user's own, named after the configuration's path, so nothing is
//! written into the project. Not on Windows in 0.18.

use std::path::{Path, PathBuf};

use rutis_loader::Loader;

/// What a host and `rutis-host status` agree to name a configuration by:
/// the configuration file `target` names (a directory's `rutis.json` when
/// it has one), else the directory, as an absolute path without links.
pub fn key(target: &Path) -> PathBuf {
    let target = std::path::absolute(target).unwrap_or_else(|_| target.to_owned());
    let target = match target.is_dir() && target.join("rutis.json").is_file() {
        true => target.join("rutis.json"),
        false => target,
    };
    target.canonicalize().unwrap_or(target)
}

/// The socket of the host running the configuration `target`.
pub fn socket(target: &Path) -> PathBuf {
    socket_in(&base(), target)
}

/// [`socket`] under the runtime directory `base`.
fn socket_in(base: &Path, target: &Path) -> PathBuf {
    directory(base).join(format!("{:016x}.sock", fnv1a(key(target).as_os_str())))
}

/// The system's runtime directory: `$XDG_RUNTIME_DIR`, else the
/// temporary directory. A `status` run with other variables (another
/// user, `sudo`) looks elsewhere.
fn base() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(std::env::temp_dir)
}

/// The user's own directory for the hosts' sockets: in the runtime
/// directory `base`, unless a socket's path there would be longer than a
/// socket may have (about 100 bytes), then in `/tmp`.
fn directory(base: &Path) -> PathBuf {
    let own = format!("rutis-host-{}", user());
    let dir = base.join(&own);
    // `<dir>/<16 hex digits>.sock`.
    match cfg!(unix) && dir.as_os_str().len() + 22 > 100 {
        true => Path::new("/tmp").join(own),
        false => dir,
    }
}

#[cfg(unix)]
fn user() -> u32 {
    // SAFETY: getuid(2) cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn user() -> u32 {
    0
}

/// FNV-1a over the path's bytes: the same in every build of rutis-host
/// (std's hasher may change between Rust versions).
fn fnv1a(text: &std::ffi::OsStr) -> u64 {
    text.as_encoded_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
        })
}

/// While held, the host answers `status` (and `call`) on its socket;
/// dropped, the socket is removed.
pub struct Serving {
    #[cfg(unix)]
    path: PathBuf,
    /// The socket file's device and inode: only that file is removed, not
    /// one another host put there since.
    #[cfg(unix)]
    file: (u64, u64),
    #[cfg(unix)]
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Serving {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            self.task.abort();
            let ours = std::fs::symlink_metadata(&self.path)
                .is_ok_and(|meta| (meta.dev(), meta.ino()) == self.file);
            if ours {
                let _ = std::fs::remove_file(&self.path);
            }
            // The directory too, once no other host's socket is in it: a
            // host leaves nothing behind.
            if let Some(dir) = self.path.parent() {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }
}

/// Answer `status` for the configuration `target` with `loader`'s status,
/// for as long as the returned value is held; and `call`, made from `calls`
/// (the host's root) when given, refused otherwise. `Err` says why it
/// cannot (another host answers for the same configuration, the directory
/// is not the user's own): the host runs on without it.
#[cfg(unix)]
pub fn serve(target: &Path, loader: Loader, calls: Option<rutis::Ctx>) -> Result<Serving, String> {
    serve_in(&base(), target, loader, calls)
}

/// [`serve`] under the runtime directory `base`.
#[cfg(unix)]
fn serve_in(
    base: &Path,
    target: &Path,
    loader: Loader,
    calls: Option<rutis::Ctx>,
) -> Result<Serving, String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    let dir = directory(base);
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("{}: {error}", dir.display())),
    }
    // Only a directory of the user's own, which no one else can enter.
    let meta =
        std::fs::symlink_metadata(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    if !meta.is_dir() || meta.uid() != user() || meta.mode() & 0o077 != 0 {
        return Err(format!(
            "{} is not a directory only this user can open",
            dir.display()
        ));
    }
    let path = socket_in(base, target);
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        return Err(format!(
            "another rutis-host answers `rutis-host status` for {}",
            key(target).display()
        ));
    }
    // One left by a host that ended without removing it.
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let meta =
        std::fs::symlink_metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let file = (meta.dev(), meta.ino());
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                // Out of descriptors, say: try again in a while, not at once.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            };
            let (loader, calls) = (loader.clone(), calls.clone());
            tokio::spawn(async move {
                let _ = answer(stream, &loader, calls.as_ref()).await;
            });
        }
    });
    Ok(Serving { path, file, task })
}

/// The longest request read: a call's arguments are in it.
#[cfg(unix)]
const REQUEST_LIMIT: u64 = 16 << 20;

/// Read one request and answer it: `status`; `call`, with its result or
/// error; or that only those are answered.
#[cfg(unix)]
async fn answer(
    stream: tokio::net::UnixStream,
    loader: &Loader,
    calls: Option<&rutis::Ctx>,
) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let mut stream = BufReader::new(stream);
    let mut request = String::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        (&mut stream).take(REQUEST_LIMIT).read_line(&mut request),
    )
    .await;
    let reply = match (read, request.trim_end_matches('\n')) {
        (Ok(Ok(_)), "status") => loader.status().to_string(),
        (Ok(Ok(_)), call) if call.starts_with("call ") => {
            let reply = match (serde_json::from_str(&call["call ".len()..]), calls) {
                (Err(error), _) => Err(format!("this call cannot be read: {error}")),
                (Ok(_), None) => Err("this rutis-host run does not accept calls; start it with \
                     rutis-host run --allow-call"
                    .to_owned()),
                (Ok(request), Some(root)) => {
                    // `rutis-host call` ends (Ctrl-C): its socket closes, and
                    // the call is dropped, which cancels it on the far end.
                    let mut gone = [0u8; 1];
                    tokio::select! {
                        reply = crate::call::call(root, loader, request) => reply,
                        _ = stream.read(&mut gone) => return Ok(()),
                    }
                }
            };
            match reply {
                Ok(result) => format!("ok\n{result}\n"),
                Err(error) => format!("error\n{error}\n"),
            }
        }
        _ => "error: this socket answers only `status` and `call`\n".to_owned(),
    };
    let mut stream = stream.into_inner();
    stream.write_all(reply.as_bytes()).await?;
    stream.shutdown().await
}

/// What the host running the configuration `target` says to `status`.
#[cfg(unix)]
pub async fn status(target: &Path) -> Result<String, String> {
    status_in(&base(), target).await
}

/// [`status`] under the runtime directory `base`; a host that does not
/// answer within 10 seconds is said to.
#[cfg(unix)]
async fn status_in(base: &Path, target: &Path) -> Result<String, String> {
    let asked = async {
        ask(base, target, "status\n").await.map_err(|error| {
            error.unwrap_or_else(|| {
                format!(
                    "no rutis-host runs {}: start it with `rutis-host run` or `rutis-host dev`, \
                     or name the configuration it runs (rutis-host status path/to/rutis.json)",
                    key(target).display()
                )
            })
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), asked)
        .await
        .unwrap_or_else(|_| {
            Err(format!(
                "the rutis-host running {} did not answer within 10s",
                key(target).display()
            ))
        })
}

/// Send the request `line` to the host running `target`, and read its
/// whole reply; `Err(None)` when no host runs it.
#[cfg(unix)]
async fn ask(base: &Path, target: &Path, line: &str) -> Result<String, Option<String>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let path = socket_in(base, target);
    let mut stream = tokio::net::UnixStream::connect(&path)
        .await
        .map_err(|_| None)?;
    let io = |error: std::io::Error| Some(error.to_string());
    stream.write_all(line.as_bytes()).await.map_err(io)?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).await.map_err(io)?;
    Ok(reply)
}

/// Why a call printed nothing on stdout: what the host said (its error),
/// or that no host answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// No host runs the configuration: what to do.
    NoHost(String),
    /// The host's error, as it is printed.
    Error(String),
}

/// Make `request` on the host running the configuration `target`, and
/// wait for its result, however long it takes (the caller decides: Ctrl-C
/// ends `rutis-host call`, which cancels the call).
#[cfg(unix)]
pub async fn call(target: &Path, request: &crate::call::Request) -> Result<String, Refused> {
    call_in(&base(), target, request).await
}

#[cfg(unix)]
async fn call_in(
    base: &Path,
    target: &Path,
    request: &crate::call::Request,
) -> Result<String, Refused> {
    let line = format!(
        "call {}\n",
        serde_json::to_string(request).map_err(|error| Refused::Error(error.to_string()))?
    );
    let reply = ask(base, target, &line)
        .await
        .map_err(|error| match error {
            Some(error) => Refused::Error(error),
            None => Refused::NoHost(format!(
                "no rutis-host run or dev is running for {} (start one, or name its \
                 configuration with --config)",
                key(target).display()
            )),
        })?;
    match reply.split_once('\n') {
        Some(("ok", result)) => Ok(result.trim_end().to_owned()),
        Some(("error", error)) => Err(Refused::Error(error.trim_end().to_owned())),
        // A host that ended during the call, or one answering only `status`.
        _ => Err(Refused::Error(format!(
            "the rutis-host running {} did not answer the call{}",
            key(target).display(),
            match reply.trim().is_empty() {
                true => String::new(),
                false => format!(": {}", reply.trim()),
            }
        ))),
    }
}

#[cfg(not(unix))]
pub fn serve(
    _target: &Path,
    _loader: Loader,
    _calls: Option<rutis::Ctx>,
) -> Result<Serving, String> {
    Err("rutis-host status is not available on Windows".into())
}

#[cfg(not(unix))]
pub async fn status(_target: &Path) -> Result<String, String> {
    Err("rutis-host status is not available on Windows in this version".into())
}

#[cfg(not(unix))]
pub async fn call(_target: &Path, _request: &crate::call::Request) -> Result<String, Refused> {
    Err(Refused::Error(
        "rutis-host call is not available on Windows".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory with `rutis.json` is named by that file, so `status` in
    /// the directory finds the host started with the file; the socket's
    /// name depends only on the path.
    #[test]
    fn a_directory_and_its_configuration_are_one_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("rutis.json"), "{}").unwrap();
        assert_eq!(key(dir.path()), key(&dir.path().join("rutis.json")));
        assert_eq!(socket(dir.path()), socket(&dir.path().join("rutis.json")));
        let other = tempfile::tempdir().unwrap();
        assert_ne!(socket(dir.path()), socket(other.path()));
        assert_eq!(fnv1a(std::ffi::OsStr::new("")), 0xcbf2_9ce4_8422_2325);
    }

    /// A host answers `status` with its loader's status, refuses `call`
    /// without its root, and anything else with an error; a second host for the same configuration does
    /// not take the socket over; once the first stops answering, its
    /// socket is gone and `status` says no host runs it.
    /// guarantee: host/status
    #[cfg(unix)]
    #[tokio::test]
    async fn the_socket_answers_status_and_call_once_per_configuration() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let base = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let loader = || {
            rutis_loader::LoaderPlugin::new(
                rutis_loader::Chain::new(),
                rutis_loader::LoaderOptions::default(),
            )
            .handle()
        };
        let serving = serve_in(base.path(), project.path(), loader(), None).unwrap();
        let said = status_in(base.path(), project.path()).await.unwrap();
        assert_eq!(said, rutis_loader::status::Status::default().to_string());

        let path = socket_in(base.path(), project.path());
        let mut stream = tokio::net::UnixStream::connect(&path).await.unwrap();
        stream.write_all(b"stop\n").await.unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await.unwrap();
        assert_eq!(
            reply,
            "error: this socket answers only `status` and `call`\n"
        );

        // Without the host's root, a call is refused.
        let request = crate::call::Request {
            service: "forecast".into(),
            method: "today".into(),
            args: Vec::new(),
        };
        let refused = call_in(base.path(), project.path(), &request)
            .await
            .unwrap_err();
        assert_eq!(
            refused,
            Refused::Error(
                "this rutis-host run does not accept calls; start it with rutis-host run --allow-call"
                    .into()
            )
        );

        let second = serve_in(base.path(), project.path(), loader(), None);
        assert!(
            second
                .as_ref()
                .is_err_and(|why| why.starts_with("another rutis-host answers")),
            "{:?}",
            second.err()
        );
        drop(serving);
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists(), "the directory is left");
        let refused = status_in(base.path(), project.path()).await.unwrap_err();
        assert!(refused.starts_with("no rutis-host runs "), "{refused}");
    }
}
