//! `apply_patches` against the JavaScript original on random layers.
//!
//! `fixtures/patch-random-expected.json` is the output of
//! cordis-plugin-include's `applyEntryPatches([], layers.flat(), warn)` —
//! version 1.0.9, the one `crates/rutis-dsh/dsh/package-lock.json` pins —
//! on 120 seeded random layer stacks; `fixtures/patch-random.mjs` generates
//! it (its header has the command). The cases are random in shape, not in
//! run: the file is the oracle, and regenerating it gives the same bytes.

use rutis_loader::patch::{apply_patches, Composed};
use rutis_loader::{Layer, Patch};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    /// `layers[i][j]`: patch `j` of layer `i`, as the JavaScript read it.
    layers: Vec<Vec<Value>>,
    /// What `applyEntryPatches` returned for them.
    rows: Vec<Value>,
    /// How many times it warned.
    warnings: usize,
}

fn layers(case: &Case) -> Vec<Layer> {
    case.layers
        .iter()
        .enumerate()
        .map(|(index, patches)| {
            let patches: Vec<Patch> = patches
                .iter()
                .map(|patch| serde_json::from_value(patch.clone()).expect("a patch"))
                .collect();
            Layer::new(format!("layer{index}"), patches)
        })
        .collect()
}

fn compose(case: &Case) -> Composed {
    apply_patches(&layers(case))
}

/// guarantee: loader/patch-parity
///
/// Every random stack composed as the JavaScript composed it: the same
/// rows, the same number of warnings. The generated cases include inserts
/// into missing and non-group targets, duplicate ids, groups replaced
/// whole, and patches aimed at rows such a replacement hid.
#[test]
fn random_layers_match_the_javascript_original() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/patch-random-expected.json")).unwrap();
    assert_eq!(cases.len(), 120, "the fixture lost cases");
    let warnings: usize = cases.iter().map(|case| case.warnings).sum();
    assert!(warnings > 400, "the cases stopped exercising warnings");
    for case in &cases {
        let composed = compose(case);
        assert_eq!(
            Value::Array(composed.rows.clone()),
            Value::Array(case.rows.clone()),
            "rows differ: {}",
            case.name
        );
        assert_eq!(
            composed.warnings.len(),
            case.warnings,
            "warning count differs: {} ({:?})",
            case.name,
            composed.warnings
        );
    }
}

/// The same layers composed twice give the same rows and warnings, and
/// composing does not change its input, so the caller may compose again.
/// guarantee: loader/patch-parity
#[test]
fn composing_is_deterministic_and_keeps_its_input() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/patch-random-expected.json")).unwrap();
    for case in &cases {
        let layers = layers(case);
        let before = layers.clone();
        let once = apply_patches(&layers);
        assert_eq!(layers, before, "inputs changed: {}", case.name);
        let twice = apply_patches(&layers);
        assert_eq!(once.rows, twice.rows, "rows differ: {}", case.name);
        assert_eq!(
            once.warnings, twice.warnings,
            "warnings differ: {}",
            case.name
        );
        assert_eq!(once.flat, twice.flat, "flat rows differ: {}", case.name);
    }
}

mod fuzz {
    use super::*;
    use proptest::prelude::*;

    fn scalar() -> impl Strategy<Value = Value> {
        prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            (-3i64..10).prop_map(Value::from),
            "[a-z]{0,4}".prop_map(Value::String),
        ]
    }

    /// Arbitrary JSON, out of shapes a patch may have: arrays of rows,
    /// objects, and the values overrides may write.
    fn json() -> impl Strategy<Value = Value> {
        scalar().prop_recursive(4, 64, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..3).prop_map(Value::Array),
                prop::collection::vec(
                    (
                        prop_oneof![
                            Just("id"),
                            Just("name"),
                            Just("insert"),
                            Just("config"),
                            Just("x")
                        ],
                        inner
                    ),
                    0..3,
                )
                .prop_map(|entries| {
                    let mut object = serde_json::Map::new();
                    for (key, value) in entries {
                        object.insert(key.to_owned(), value);
                    }
                    Value::Object(object)
                }),
            ]
        })
    }

    proptest! {
        /// guarantee: loader/patch-parity
        ///
        /// Patches of any JSON shape — a malformed one is dropped — compose
        /// without a panic; every warning names a layer and a patch that
        /// exist; and the inputs are not changed. This is the arena's index
        /// arithmetic (#631): ids may repeat, be missing, or arrive with an
        /// insert into a target that is not there.
        #[test]
        fn any_patch_json_composes_without_a_panic(
            layers in prop::collection::vec(prop::collection::vec(json(), 0..5), 0..4),
        ) {
            let layers: Vec<Layer> = layers
                .into_iter()
                .enumerate()
                .map(|(index, patches)| {
                    let patches: Vec<Patch> = patches
                        .into_iter()
                        .filter_map(|patch| serde_json::from_value(patch).ok())
                        .collect();
                    Layer::new(format!("layer{index}"), patches)
                })
                .collect();
            let before = layers.clone();
            let composed = apply_patches(&layers);
            prop_assert_eq!(&layers, &before, "inputs changed");
            prop_assert_eq!(&composed, &apply_patches(&layers), "not deterministic");
            let counts: Vec<usize> = layers.iter().map(|layer| layer.patches.len()).collect();
            for warning in &composed.warnings {
                let layer: usize = warning
                    .layer
                    .strip_prefix("layer")
                    .and_then(|index| index.parse().ok())
                    .expect("a layer");
                prop_assert!(
                    warning.index < counts[layer],
                    "{} names no patch of {}",
                    warning.index,
                    warning.layer
                );
            }
        }
    }
}
