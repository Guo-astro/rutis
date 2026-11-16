//! The `!!js` expressions of dsh's profile (#631): lexed, parsed and
//! evaluated with a fixed environment and no services, so arbitrary text is
//! an error, never a panic (`rutis_dsh::profile::expr`).
#![no_main]

use std::collections::HashMap;

use rutis_dsh::profile::expr::{node_platform, Environment, JsSubset};
use rutis_loader::resolve::{ExprScope, Expressions};
use rutis_loader::ServiceCatalog;

/// A profile's expressions are small; a larger input is not read.
const MAX_LEN: usize = 64 * 1024;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    if source.len() > MAX_LEN {
        return;
    }
    // `Some` for the variables and the working directory: evaluation reads
    // no live process state. No host functions: a call to one is an error.
    let expressions = JsSubset::new(Environment {
        vars: Some(HashMap::new()),
        cwd: Some("/".into()),
        platform: node_platform().into(),
        functions: HashMap::new(),
    });
    let catalog = ServiceCatalog::new();
    let scope = ExprScope::new(None, &catalog);
    let _ = expressions.evaluate(source, &scope);
});
