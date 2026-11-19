//! The entry-list YAML dialect against js-yaml, on every bundle patch file
//! dsh ships, plus round trips through `Document`.

mod support;

use rutis_dsh::profile::yaml::{self, Document};
use serde_json::{json, Value};

#[test]
fn parses_like_js_yaml() {
    let Some(files) = support::bundle_patch_files() else {
        eprintln!("skipped: the dsh npm project is not installed");
        return;
    };
    assert!(files.len() > 5, "{files:?}");
    let expected = support::node(&["yaml"], files.iter().map(|f| f.to_str().unwrap()));
    for case in expected.as_array().unwrap() {
        let file = case["file"].as_str().unwrap();
        let ours = yaml::parse(&std::fs::read_to_string(file).unwrap()).unwrap();
        assert_eq!(ours, case["value"], "{file}");
    }
}

#[test]
fn rewriting_unchanged_items_keeps_the_text() {
    let Some(files) = support::bundle_patch_files() else {
        return;
    };
    for file in files {
        let source = std::fs::read_to_string(&file).unwrap();
        let doc = Document::parse(&source).unwrap();
        let values = doc.values();
        assert_eq!(
            doc.render(&values, |v| v.clone()),
            source,
            "{}",
            file.display()
        );
        // Fresh output reads back to the same values.
        let fresh = doc.render_fresh(&values);
        assert_eq!(
            yaml::parse(&fresh).unwrap(),
            Value::Array(values),
            "{}",
            file.display()
        );
    }
}

#[test]
fn scalars_and_expressions_like_js_yaml() {
    if support::project().is_none() {
        return;
    }
    let source = "\
- a: !!js process.cwd()
  b: !!js \"x ?? 'y'\"
  c: yes
  d: ~
  e: 0x1F
  f: 1e3
  g: '010'
  h: 010
  i: [1, two, {k: v}]
  'j k': true
  l: 1_000
  m: .5
  n: +12
  o: 0o17
  p: -0.5e-2
  q: .inf
  r: 1.
  s: \"multi\\nline\"
  t: !!str 12
  u: &x {a: 1}
  v: *x
";
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("s.yml");
    std::fs::write(&file, source).unwrap();
    let expected = support::node(&["yaml"], [file.to_str().unwrap()]);
    assert_eq!(yaml::parse(source).unwrap(), expected[0]["value"]);
    assert_eq!(
        yaml::parse(source).unwrap()[0]["a"],
        json!({ "__jsExpr": "process.cwd()" })
    );
}

#[test]
fn edits_keep_comments_of_untouched_items() {
    let source = "\
# Your patch layer.
# Two lines of header.

# about a
- id: a
  disabled: true   # trailing note

# about b
- id: b
  config:
    level: 1
";
    let doc = Document::parse(source).unwrap();
    let mut values = doc.values();
    values[1] =
        json!({ "id": "b", "config": { "level": 2, "expr": { "__jsExpr": "process.env.X" } } });
    values.push(json!({ "insert": [{ "id": "c", "name": "@scope/pkg", "config": {} }] }));
    let rendered = doc.render(&values, |v| v.clone());
    assert!(rendered.starts_with("# Your patch layer.\n# Two lines of header.\n\n# about a\n- id: a\n  disabled: true   # trailing note\n"), "{rendered}");
    assert!(
        !rendered.contains("# about b"),
        "the changed item is rewritten: {rendered}"
    );
    assert_eq!(yaml::parse(&rendered).unwrap(), Value::Array(values));

    // An empty template keeps its header.
    let empty = Document::parse("# header\n[]\n").unwrap();
    assert_eq!(empty.render(&[], |v| v.clone()), "# header\n[]\n");
    let one = empty.render(&[json!({ "id": "x", "disabled": true })], |v| v.clone());
    assert_eq!(one, "# header\n- id: x\n  disabled: true\n");
}

#[test]
fn line_spans_that_do_not_fit_are_rewritten() {
    // #1235: the YAML parser counts `\r` as a line break while
    // `split_inclusive('\n')` does not, so an item's start line can be past
    // the last line. #1237: an anchor can give an item an earlier line than
    // the one before it. Neither may be sliced into the source lines.
    for source in ["\r-\r ", "\r- a\r- b", "- &i\n-", "#\r-\r- \0\n "] {
        let doc = Document::parse(source).expect("a document");
        let fresh = doc.render_fresh(&doc.values());
        Document::parse(&fresh).expect("the rendered document");
    }
}

#[test]
fn bare_sequence_marker_is_part_of_the_item_span() {
    for source in ["-\n n", "-\n  key: value", "-\n  - nested"] {
        let doc = Document::parse(source).expect("a document");
        let values = doc.values();

        assert_eq!(doc.render(&values, |v| v.clone()), format!("{source}\n"));
        let fresh = doc.render_fresh(&values);
        assert_eq!(
            yaml::parse(&fresh).expect("fresh output"),
            Value::Array(values),
            "{source:?}"
        );
    }
}

/// A sign inside a radix prefix is not a number, and must not panic:
/// `i64::from_str_radix` reads it, and negating `i64::MIN` overflowed. The
/// `dsh_profile_yaml` fuzz target found `- -0o-1000000000000000000000`
/// (2026-11-19, #1080).
#[test]
fn a_sign_after_a_radix_prefix_is_a_string_not_a_panic() {
    for source in [
        "- -0o-1000000000000000000000",
        "- 0o-1000000000000000000000",
        "- -0o-1",
        "- 0x-1",
        "- 0b-1",
        "- 0x8000000000000000",
        "- -1{{ie: il",
    ] {
        let _ = yaml::parse(source);
        let _ = Document::parse(source);
    }
    // What is not a radix number is a string.
    assert_eq!(
        yaml::parse("-0o-1").unwrap(),
        Value::String("-0o-1".to_owned())
    );
}
