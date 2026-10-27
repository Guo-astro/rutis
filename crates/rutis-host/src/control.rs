//! `rutis-host status`: a host started by `run` or `dev` answers it on a
//! local socket that only its user can open (design §5.3). The socket
//! answers one request, `status`, with [`Loader::status`] as printed; it
//! changes nothing. It sits in the system's runtime directory
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

/// While held, the host answers `status` on its socket; dropped, the
/// socket is removed.
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
/// for as long as the returned value is held. `Err` says why it cannot
/// (another host answers for the same configuration, the directory is not
/// the user's own): the host runs on without it.
#[cfg(unix)]
pub fn serve(target: &Path, loader: Loader) -> Result<Serving, String> {
    serve_in(&base(), target, loader)
}

/// [`serve`] under the runtime directory `base`.
#[cfg(unix)]
fn serve_in(base: &Path, target: &Path, loader: Loader) -> Result<Serving, String> {
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
            let loader = loader.clone();
            tokio::spawn(async move {
                let _ = answer(stream, &loader).await;
            });
        }
    });
    Ok(Serving { path, file, task })
}

/// Read one request and answer it: `status`, or that only `status` is
/// answered.
#[cfg(unix)]
async fn answer(stream: tokio::net::UnixStream, loader: &Loader) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    let mut stream = stream;
    let mut request = String::new();
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        BufReader::new((&mut stream).take(64)).read_line(&mut request),
    )
    .await;
    let reply = match (read, request.trim()) {
        (Ok(Ok(_)), "status") => loader.status().to_string(),
        _ => "error: this socket answers only `status`\n".to_owned(),
    };
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
    tokio::time::timeout(std::time::Duration::from_secs(10), ask(base, target))
        .await
        .unwrap_or_else(|_| {
            Err(format!(
                "the rutis-host running {} did not answer within 10s",
                key(target).display()
            ))
        })
}

#[cfg(unix)]
async fn ask(base: &Path, target: &Path) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let path = socket_in(base, target);
    let mut stream = tokio::net::UnixStream::connect(&path).await.map_err(|_| {
        format!(
            "no rutis-host runs {}: start it with `rutis-host run` or `rutis-host dev`, or name \
             the configuration it runs (rutis-host status path/to/rutis.json)",
            key(target).display()
        )
    })?;
    stream
        .write_all(b"status\n")
        .await
        .map_err(|error| error.to_string())?;
    let mut reply = String::new();
    stream
        .read_to_string(&mut reply)
        .await
        .map_err(|error| error.to_string())?;
    Ok(reply)
}

#[cfg(not(unix))]
pub fn serve(_target: &Path, _loader: Loader) -> Result<Serving, String> {
    Err("rutis-host status is not available on Windows".into())
}

#[cfg(not(unix))]
pub async fn status(_target: &Path) -> Result<String, String> {
    Err("rutis-host status is not available on Windows in this version".into())
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

    /// A host answers `status` with its loader's status, and anything
    /// else with an error; a second host for the same configuration does
    /// not take the socket over; once the first stops answering, its
    /// socket is gone and `status` says no host runs it.
    /// guarantee: host/status
    #[cfg(unix)]
    #[tokio::test]
    async fn the_socket_answers_status_only_and_once_per_configuration() {
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
        let serving = serve_in(base.path(), project.path(), loader()).unwrap();
        let said = status_in(base.path(), project.path()).await.unwrap();
        assert_eq!(said, rutis_loader::status::Status::default().to_string());

        let path = socket_in(base.path(), project.path());
        let mut stream = tokio::net::UnixStream::connect(&path).await.unwrap();
        stream.write_all(b"stop\n").await.unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await.unwrap();
        assert_eq!(reply, "error: this socket answers only `status`\n");

        let second = serve_in(base.path(), project.path(), loader());
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
