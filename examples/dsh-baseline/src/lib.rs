//! Typed rutis bindings for published dsh Cordis plugins, generated from
//! their declarations during the normal Cargo build. The mounts are listed
//! in Cargo.toml; each becomes a module exposing `Plugin`, `Config` and the
//! native service proxy types. The events the plugins emit reach Rust
//! listeners by name ([`events`]).
#![cfg(all(unix, dsh_baseline))]

rutis_bridge::include_mounts!();

/// Events of the mounted plugins, as Rust types: a listener of one hears
/// the plugin's emissions of its name (no declaration in the mount since
/// rutis 0.13).
pub mod events {
    use serde::{Deserialize, Serialize};

    /// `credentials/reference-updated (ref)`: a committed change to a
    /// provider-managed credential reference.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct CredentialsReferenceUpdated(pub String);

    impl rutis::Event for CredentialsReferenceUpdated {
        const NAME: &'static str = "credentials/reference-updated";
        type Value = ();
    }

    /// `credentials/record-updated (key)`: a committed change to a stored
    /// credential record.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct CredentialsRecordUpdated(pub String);

    impl rutis::Event for CredentialsRecordUpdated {
        const NAME: &'static str = "credentials/record-updated";
        type Value = ();
    }

    /// `system-prompt/change ()`: the system prompt's sections changed (no
    /// arguments).
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct SystemPromptChange;

    impl rutis::Event for SystemPromptChange {
        const NAME: &'static str = "system-prompt/change";
        type Value = ();
    }
}
