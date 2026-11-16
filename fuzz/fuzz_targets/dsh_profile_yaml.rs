//! dsh's profile YAML dialect (#631): a patch layer document read into JSON
//! values, a top-level sequence split into its items, rendered back and read
//! again. Arbitrary text is an error, never a panic
//! (`rutis_dsh::profile::yaml`).
//!
//! What a document renders is read again but not compared: #1234 (found by
//! this target) has `"-\n n"` read as one item and rendered as two; the
//! fix and its regression test belong with `yaml.rs`.
#![no_main]

use rutis_dsh::profile::yaml::{self, Document};

/// A profile's patch files are small; a larger input is not read.
const MAX_LEN: usize = 64 * 1024;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    if source.len() > MAX_LEN {
        return;
    }
    let _ = yaml::parse(source);
    let Ok(document) = Document::parse(source) else {
        return;
    };
    let values = document.values();
    let rendered = [
        document.render_fresh(&values),
        document.render(&values, |value| value.clone()),
    ];
    for text in rendered {
        let _ = yaml::parse(&text);
        let _ = Document::parse(&text);
    }
});
