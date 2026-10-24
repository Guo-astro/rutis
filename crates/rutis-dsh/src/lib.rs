//! Runs dsh in a rutis host, with the model calls of dsh plugins served by
//! aimux-llm in this process.
//!
//! - [`web`]: the dsh web UI. The launcher (`dsh/launcher.ts`) starts a dsh
//!   profile in the mounted Context; `rutis-dsh up` runs it.
//! - [`agent`]: the dsh agent loop without a UI, for driving agents from Rust.
//!
//! Both mounts depend on the `aimux` service: register it with
//! [`provide_web_aimux`] / [`provide_agent_aimux`] before mounting. They are
//! built only when the npm project is installed (Unix).
//!
//! - [`profile`]: dsh's profile configuration on rutis-loader — the patch
//!   layers, the `!!js` expressions, and the user layer's storage. It needs
//!   no Node.

pub mod profile;

/// The events the launcher (`dsh/launcher.ts`) emits from the [`web`]
/// mount. They reach Rust listeners by name, as every event does since
/// rutis 0.13 (no declaration in the mount).
pub mod events {
    use serde::{Deserialize, Serialize};

    /// `rutis-dsh/ready`: the profile started; the dsh-llm routes now
    /// available.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Ready(pub Vec<String>);

    impl rutis::Event for Ready {
        const NAME: &'static str = "rutis-dsh/ready";
        type Value = ();
    }

    /// `rutis-dsh/startup-failed`: the profile failed to start, and why; the
    /// mount stays up until the host disposes it.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct StartupFailed(pub String);

    impl rutis::Event for StartupFailed {
        const NAME: &'static str = "rutis-dsh/startup-failed";
        type Value = ();
    }

    /// `rutis-dsh/exit`: the dsh app asked to exit with this code (`--help`,
    /// a usage error).
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct Exit(pub f64);

    impl rutis::Event for Exit {
        const NAME: &'static str = "rutis-dsh/exit";
        type Value = ();
    }
}

#[cfg(all(unix, dsh_installed))]
rutis_bridge::include_mounts!();

#[cfg(all(unix, dsh_installed))]
mod aimux;

#[cfg(all(unix, dsh_installed))]
use std::sync::Arc;

#[cfg(all(unix, dsh_installed))]
use aimux_llm::LlmService;
#[cfg(all(unix, dsh_installed))]
use rutis::{Ctx, Disposer, Error};

#[cfg(all(unix, dsh_installed))]
pub use aimux::AimuxBridge;

#[cfg(all(unix, dsh_installed))]
aimux::serve_aimux!(web);
#[cfg(all(unix, dsh_installed))]
aimux::serve_aimux!(agent);

/// Registers `service` as the `aimux` service of the [`web`] mount.
#[cfg(all(unix, dsh_installed))]
pub fn provide_web_aimux(ctx: &Ctx, service: Arc<dyn LlmService>) -> Result<Disposer, Error> {
    web::provide_aimux(ctx, AimuxBridge::new(service))
}

/// Registers `service` as the `aimux` service of the [`agent`] mount.
#[cfg(all(unix, dsh_installed))]
pub fn provide_agent_aimux(ctx: &Ctx, service: Arc<dyn LlmService>) -> Result<Disposer, Error> {
    agent::provide_aimux(ctx, AimuxBridge::new(service))
}
