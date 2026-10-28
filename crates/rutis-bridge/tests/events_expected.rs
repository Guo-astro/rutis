//! The unified events expectation file (`events.expected.json`, #464) has
//! the shape every implementation relies on: each case parses, names only
//! labels, scopes and languages it has, and has placements for every
//! runtime language. Running the cases is
//! `crates/rutis-loader/tests/events_conformance.rs` and the testing tools'.
#![cfg(feature = "testing")]

use rutis_bridge::runtime::testing::events::{
    argument, combinations, expected, placements, problems, runtime_languages, tool_combo,
    y_languages, Coverage,
};

#[test]
fn the_expectation_file_is_well_formed() {
    let expected = expected();
    let problems = problems(&expected);
    assert!(problems.is_empty(), "{problems:#?}");
    // The starter cases of the spec (§13.3) are all there; `rust/no-wire`
    // became `rust/unit-event` (Rust events cross by serde, always).
    let starter = [
        "emit/reaches-all",
        "emit/order-per-listener",
        "emit/order-across-fast-path",
        "emit/error-isolated",
        "parallel/all-ok",
        "parallel/errors-in-registration-order",
        "parallel/one-error-is-aggregated",
        "serial/first-result",
        "serial/no-result",
        "serial/zero-is-a-result",
        "serial/error-stops",
        "waterfall/chain",
        "waterfall/veto",
        "waterfall/terminal",
        "waterfall/no-listeners",
        "waterfall/end-without-terminal",
        "waterfall/next-twice",
        "waterfall/error",
        "bail/first-result",
        "bail/async-listener",
        "bail/awaitable-from-plain-function",
        "bail/call-back-into-waiting-emitter",
        "bail/wait-cycle",
        "scope/flat-bus",
        "filter/keeps-and-sees",
        "filter/cordis-this-arg",
        "filter/global-skips-filter",
        "order/prepend",
        "values/every-tag",
        "values/function-argument",
        "unload/waits-and-cancels",
        "unload/not-called-after",
        "registration/loaded-row-hears",
        "cancel/emitter-gives-up",
        "disconnect/in-flight-fails-once",
        "disconnect/emit-reported",
        "disconnect/node-link",
        "node/not-shared",
        "rutis/row",
        "rutis/service",
        "rutis/runtime",
        "reserved/rutis-emit",
        "fast-path/local-only",
        "rust/typed-args",
        "rust/unit-event",
        "rust/next-outside-waterfall",
    ];
    assert_eq!(starter.len(), 46);
    for case in starter {
        assert!(
            expected.cases.iter().any(|c| c.case == case),
            "{case} is missing"
        );
    }
}

/// Every case runs in some placement of the real runtimes; printed with
/// `--nocapture`: how many placements per language under test, and how
/// many cases each testing tool runs.
#[test]
fn every_case_runs_somewhere() {
    let expected = expected();
    for x in runtime_languages(&expected) {
        let ys = y_languages(&expected, x);
        let total: usize = expected
            .cases
            .iter()
            .map(|case| combinations(case, x, &ys).unwrap().len())
            .sum();
        let sampled: usize = expected
            .cases
            .iter()
            .map(|case| placements(case, x, &ys, Coverage::Sampled).unwrap().len())
            .sum();
        println!("X={x}: {total} placements, {sampled} sampled");
        // By the part of the case id before its `/`: what the nightly run
        // of every placement shards by (stress.yml, `events-all`).
        let mut by_kind = std::collections::BTreeMap::<&str, usize>::new();
        for case in &expected.cases {
            let kind = case.case.split('/').next().unwrap_or_default();
            *by_kind.entry(kind).or_default() += combinations(case, x, &ys).unwrap().len();
        }
        println!("  {by_kind:?}");
    }
    for case in &expected.cases {
        let anywhere = runtime_languages(&expected).iter().any(|x| {
            !combinations(case, x, &y_languages(&expected, x))
                .unwrap()
                .is_empty()
        });
        assert!(anywhere, "{} runs in no placement", case.case);
    }
    // Each testing tool checks it ran as many cases as `toolCases` says,
    // and `toolCases` is what the cases give each tool's language.
    let tools: Vec<&str> = expected.tool_cases.keys().map(String::as_str).collect();
    assert_eq!(
        tools,
        ["bun", "go", "kotlin", "node-leaf", "python", "swift"],
        "toolCases"
    );
    let counted: std::collections::BTreeMap<String, usize> = tools
        .iter()
        .map(|language| {
            let tool = expected
                .cases
                .iter()
                .filter(|case| tool_combo(case, language).is_some())
                .count();
            println!("{language} testing tool: {tool} cases");
            (language.to_string(), tool)
        })
        .collect();
    assert_eq!(
        expected.tool_cases, counted,
        "toolCases in events.expected.json"
    );
}

/// A tagged value the file writes wrong is an error of the file, not a
/// record some case passes with.
#[test]
#[should_panic(expected = "a bad tagged value")]
fn a_bad_tagged_value_in_the_file_panics() {
    argument(&serde_json::json!([{ "$instant": "yesterday" }]));
}

#[test]
#[should_panic(expected = "a bad tagged value")]
fn an_unknown_tag_in_the_file_panics() {
    argument(&serde_json::json!({ "$instnat": "2026-10-15T03:22:15.123Z" }));
}
