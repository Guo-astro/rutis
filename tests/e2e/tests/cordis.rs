//! S6, published Cordis plugins mounted from `rutis.json` (#190; risk F8,
//! and F4 in the nightly run on the newest versions): the published dsh
//! plugins of `node/baseline`, installed from its lockfile (`npm --prefix
//! node/baseline ci`), are rows that `rutis-host` runs in its Node runtime,
//! and the results are compared with the same plugins in native Cordis
//! (`node/baseline/hosted.mjs`, `native.mjs`). The project links that npm
//! project's `node_modules`, as a user's project has its own.
//!
//! The first scenario runs today. `dsh-persona` depends on `systemPrompt`,
//! which a Python row provides on the rutis side: `check` lists what each
//! row uses and provides; `run` starts every row; the sections the persona
//! registers, and every call it makes on the service, are the native ones;
//! withdrawing the service (the row disabled in `rutis.json`, read again
//! with `--watch-config`) stops the persona, which unregisters its sections
//! as in Cordis, and restoring it starts the persona again. A Cordis plugin
//! that breaks boundary rule 8 (an error nothing catches) ends the Node
//! runtime: the host says so and how, the rows there wait, the Python rows
//! go on. The residue checks end it (Q5.4.2).
//!
//! The second scenario: the published plugins use each other's services
//! and answer a probe as in native Cordis (`node/baseline/scenarios.mjs`,
//! #428). Rows of the same Node runtime resolve each other's services in
//! Cordis; what they provide reaches rutis without any declaration, with
//! the methods each service has; `check` says such a dependency may be
//! provided at runtime instead of failing.

use std::path::{Path, PathBuf};

use rutis_e2e::{repo, Host, Lang, Scenario};
use serde_json::{json, Map, Value};

/// The rutis side's `systemPrompt`: it records each call the persona makes
/// (one JSON array a line, in `config["trace"]`), as `hosted.mjs`'s host
/// service does in native Cordis, and shows the sections registered
/// through `prompts`.
const PROMPTS: &str = r#"
import json

# The orders dsh-system-prompt's registry gives the persona's sections.
ORDERS = {"DEPLOYMENT_PERSONA_PREFIX": 0, "DEPLOYMENT_PERSONA_SUFFIX": 10200}

provides = {
    "systemPrompt": {"section": "sync", "getSectionOrder": "sync", "suppressRuntimeContext": "sync"},
    "prompts": {"sections": "sync"},
}


def apply(ctx, config):
    sections = []

    def record(*call):
        with open(config["trace"], "a", encoding="utf-8") as file:
            file.write(json.dumps(list(call)) + "\n")

    class SystemPrompt:
        def section(self, section):
            record("section", section["name"], section.get("order"), section.get("text"))
            sections.append(section)

            def dispose():
                record("dispose", section["name"])
                sections.remove(section)

            return dispose

        def getSectionOrder(self, name):
            record("getSectionOrder", name)
            return ORDERS[name]

        def suppressRuntimeContext(self):
            record("suppressRuntimeContext")

    class Prompts:
        def sections(self):
            ordered = sorted(sections, key=lambda section: section["order"])
            return [{key: section[key] for key in ("name", "order", "text")} for section in ordered]

    ctx.provide("systemPrompt", SystemPrompt())
    ctx.provide("prompts", Prompts())
"#;

/// A Cordis plugin, in a package of the project's own that declares what it
/// provides to rutis (`rutis.provides`), that breaks boundary rule 8:
/// `crash` throws where nothing catches it.
const CRASHER: &str = r#"
export const name = 'crasher'
export function apply(ctx) {
  ctx.provide('crasher', {
    crash(reason) {
      setTimeout(() => { throw new Error(reason) })
      return 'scheduled'
    },
  })
}
"#;

/// The published plugins that use nothing, as rows: id, package, config
/// (with `$DIR`, the scenario's directory).
const PLAIN: [(&str, &str, &str); 5] = [
    ("invariants", "@deepseek-ai/dsh-invariants", "{}"),
    (
        "credentials",
        "@deepseek-ai/dsh-credentials-local",
        r#"{ "dshHome": "$DIR/dsh-home", "watch": false }"#,
    ),
    ("fs", "@deepseek-ai/dsh-fs-local", r#"{ "cwd": "$DIR" }"#),
    ("jobs", "@deepseek-ai/dsh-jobs-local", "{}"),
    ("commands", "@deepseek-ai/dsh-commands", "{}"),
];

/// `node/baseline`, whose packages the scenarios run.
fn baseline() -> PathBuf {
    let dir = repo().join("node/baseline");
    assert!(
        dir.join("node_modules/@deepseek-ai/cordis").is_dir(),
        "the published Cordis plugins are not installed: run npm --prefix node/baseline ci"
    );
    dir
}

/// The installed version of `package`: the lockfile's, or in the nightly
/// run on the newest versions, the newest.
fn installed(package: &str) -> String {
    let manifest = baseline()
        .join("node_modules")
        .join(package)
        .join("package.json");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest).expect("the package's manifest"))
            .expect("package.json");
    manifest["version"].as_str().expect("a version").to_owned()
}

/// The project's Node side: its `package.json`, and `node_modules` linked
/// to `node/baseline`'s, entry by entry (the scenario may add its own).
fn node_project(s: &Scenario) {
    s.write_json(
        "package.json",
        &json!({ "name": "e2e-cordis", "private": true, "type": "module" }),
    );
    let modules = baseline().join("node_modules");
    for entry in std::fs::read_dir(&modules).expect("node/baseline/node_modules") {
        let name = entry.expect("an entry").file_name();
        let name = name.to_string_lossy();
        if !name.starts_with('.') {
            s.link(format!("node_modules/{name}"), &modules.join(&*name));
        }
    }
}

/// Run `node <script> <args>` from `node/baseline` and read the JSON it
/// prints.
fn node(s: &Scenario, script: &str, args: &[&str]) -> Value {
    // Without the `\\?\` of a canonical Windows path: `scenarios.mjs`
    // prints only when `process.argv[1]` is its own path.
    let script = baseline().join(script).display().to_string();
    let script = PathBuf::from(script.strip_prefix(r"\\?\").unwrap_or(&script));
    let mut program = s.program(
        "",
        "node",
        std::iter::once(script.as_os_str()).chain(args.iter().map(|arg| arg.as_ref())),
        &[],
    );
    let status = program.wait_exit();
    let output: String = program.lines().into_iter().map(|line| line.text).collect();
    assert!(
        status.success(),
        "node {}: {status}\n{output}",
        script.display()
    );
    serde_json::from_str(&output)
        .unwrap_or_else(|error| panic!("node {}: {error}\n{output}", script.display()))
}

/// The calls recorded in the trace since `*seen` of them were read.
fn recorded(trace: &Path, seen: &mut usize) -> Vec<Value> {
    let text = std::fs::read_to_string(trace).unwrap_or_default();
    let calls: Vec<Value> = text
        .lines()
        .skip(*seen)
        .map(|line| serde_json::from_str(line).expect("a recorded call"))
        .collect();
    *seen += calls.len();
    calls
}

/// The lines of `host`'s output so far.
fn output(host: &Host) -> Vec<String> {
    host.lines().into_iter().map(|line| line.text).collect()
}

/// risk: F8 (mounted from rutis.json, a Cordis plugin behaves otherwise
/// than mounted by a library, or than in Cordis)
///
/// guarantee: boundary/5.8 (an error a Cordis plugin leaves uncaught is
/// reported, under `run` as from an unknown row, and the Node runtime goes
/// on), host/row-state-lines, host/runtime-state-lines
#[test]
fn published_cordis_plugins_run_from_rutis_json_as_in_native_cordis() {
    let s = Scenario::new("cordis");
    let native = node(&s, "hosted.mjs", &[]);
    let phases = &native["phases"];

    // 1. The project: the published plugins as rows, the persona among
    //    them; `systemPrompt` from a Python row; a Cordis plugin of the
    //    project's own; a Python probe for each service it calls.
    node_project(&s);
    s.write("prompts.py", PROMPTS);
    s.write_json(
        "crasher/package.json",
        &json!({
            "name": "crasher",
            "type": "module",
            "rutis": { "provides": { "crasher": { "crash": "async" } } },
        }),
    );
    s.write("crasher/index.mjs", CRASHER);
    let reader = s.probe("reader", Lang::Python, &["prompts"]);
    let breaker = s.probe("breaker", Lang::Python, &["crasher"]);
    let dir = s.dir().display().to_string().replace('\\', "/");
    let trace = s.dir().join("trace.jsonl");
    let mut rows: Vec<Value> = PLAIN
        .iter()
        .map(|(id, package, config)| {
            let config: Value = serde_json::from_str(&config.replace("$DIR", &dir)).unwrap();
            json!({ "id": id, "name": package, "config": config })
        })
        .collect();
    let prompts = json!({ "id": "prompts", "name": "py:prompts", "config": { "trace": trace } });
    rows.extend([
        json!({ "id": "persona", "name": "@deepseek-ai/dsh-persona", "config": native["config"] }),
        prompts.clone(),
        json!({ "id": "crasher", "name": "./crasher/index.mjs" }),
        reader.row(),
        breaker.row(),
    ]);
    let config = |prompts: &Value| {
        let mut rows = rows.clone();
        let at = rows.iter().position(|row| row["id"] == "prompts").unwrap();
        rows[at] = prompts.clone();
        serde_json::to_string_pretty(&json!({
            "runtimes": { "node": {}, "py": {} },
            "rows": rows,
        }))
        .unwrap()
    };
    s.write("rutis.json", config(&prompts));

    // 2. `check` lists each row's version, what it uses and what it
    //    provides; every row can run.
    let mut check = s.host(["check", "rutis.json"]);
    let status = check.wait_exit();
    let printed = output(&check).join("\n");
    assert!(status.success(), "check: {status}\n{printed}");
    for (id, package, _) in
        PLAIN
            .iter()
            .copied()
            .chain([("persona", "@deepseek-ai/dsh-persona", "")])
    {
        let row = format!(
            "{id} ({package}): ok\n  version: \"{}\"\n",
            installed(package)
        );
        assert!(printed.contains(&row), "check, row {id}:\n{printed}");
    }
    let persona = printed.split("persona (").nth(1).unwrap();
    assert!(
        persona.contains("inject: [\"systemPrompt\"]"),
        "check, what persona uses:\n{printed}"
    );
    let python = printed.split("prompts (py:prompts): ok").nth(1).unwrap();
    let provides = python
        .lines()
        .find(|line| line.contains("provides:"))
        .unwrap();
    assert!(
        provides.contains("\"systemPrompt\"") && provides.contains("\"getSectionOrder\":\"sync\""),
        "check, what prompts provides:\n{printed}"
    );
    let crasher = printed.split("crasher (").nth(1).unwrap();
    assert!(
        crasher.contains("provides: {\"crasher\":{\"crash\":\"async\"}}"),
        "check, what crasher provides:\n{printed}"
    );

    // 3. `run`: every row runs. The persona registers its sections with the
    //    rutis side's `systemPrompt`, making the native calls; the probe
    //    reads the sections native Cordis has.
    let mut host = s.host(["run", "--watch-config", "rutis.json"]);
    let ids: Vec<&str> = PLAIN.iter().map(|(id, ..)| *id).collect();
    for id in ids
        .iter()
        .chain(&["persona", "prompts", "crasher", "reader", "breaker"])
    {
        host.expect(&format!("{id}: running"));
    }
    reader.started(&mut host);
    breaker.started(&mut host);
    let mut seen = 0;
    assert_eq!(
        recorded(&trace, &mut seen),
        phases["start"]["calls"].as_array().unwrap()[..]
    );
    assert_eq!(
        reader.call(&mut host, "prompts", "sections", json!([])),
        Ok(phases["start"]["sections"].clone())
    );

    // 4. The rutis side's service withdrawn: the persona stops by Cordis's
    //    rules, unregistering its sections (as in Cordis, in reverse); the
    //    probe that reads them stops too, nothing else does. Restored: the
    //    persona starts again and registers them again.
    host.skip_written();
    let mark = host.lines().len();
    let mut disabled = prompts.clone();
    disabled["disabled"] = json!(true);
    s.replace("rutis.json", config(&disabled));
    host.expect("rutis-host: reloaded rutis.json (1 row changed)");
    host.expect("persona: waiting for ");
    host.expect("prompts: disabled");
    reader.event(&mut host, "stopped");
    assert_eq!(
        recorded(&trace, &mut seen),
        phases["withdraw"]["calls"].as_array().unwrap()[..]
    );
    s.replace("rutis.json", config(&prompts));
    host.expect("rutis-host: reloaded rutis.json (1 row changed)");
    host.expect("prompts: running");
    host.expect("persona: running");
    reader.started(&mut host);
    assert_eq!(
        recorded(&trace, &mut seen),
        phases["restore"]["calls"].as_array().unwrap()[..]
    );
    assert_eq!(
        reader.call(&mut host, "prompts", "sections", json!([])),
        Ok(phases["restore"]["sections"].clone())
    );
    let moved: Vec<String> = output(&host)
        .into_iter()
        .skip(mark)
        .filter(|line| {
            ids.iter()
                .chain(&["crasher", "breaker"])
                .any(|id| line.starts_with(&format!("{id}: ")))
        })
        .collect();
    assert!(
        moved.is_empty(),
        "rows that do not use the service changed: {moved:?}"
    );

    // 5. Boundary rule 8 broken: an error a Cordis plugin leaves uncaught
    //    (#688). Under `run` the runtime does not trace rows: it reports
    //    the error as from an unknown row and goes on. No row fails or
    //    waits, the plugin answers again, and the Python rows go on.
    host.skip_written();
    let mark = host.lines().len();
    for reason in ["uncaught in crasher", "uncaught again"] {
        assert_eq!(
            breaker.call(&mut host, "crasher", "crash", json!([reason])),
            Ok(json!("scheduled"))
        );
        host.expect(&format!(
            "runtime node: an unhandled error from an unknown row: Error: {reason}"
        ));
    }
    assert_eq!(
        reader.call(&mut host, "prompts", "sections", json!([])),
        Ok(phases["restore"]["sections"].clone()),
        "the Python rows go on"
    );
    let changed: Vec<String> = output(&host)
        .into_iter()
        .skip(mark)
        .filter(|line| {
            line.contains(": failed") || line.contains(": waiting for ") || line.contains(": down")
        })
        .collect();
    assert!(changed.is_empty(), "rows or runtimes changed: {changed:?}");

    // 6. Stopped; nothing is left (the residue checks).
    host.ctrl_c();
    let status = host.wait_exit();
    assert_eq!(status.code(), Some(0), "after Ctrl-C: {status}");
    s.finish();
}

/// One call of a `scenarios.mjs` target.
struct Call {
    method: String,
    args: Value,
    capture: Option<String>,
}

/// `value` with each string `"$name"` replaced by what was captured as
/// `name` (as `scenarios.mjs`'s `substitute`).
fn substitute(value: &Value, captured: &Map<String, Value>) -> Value {
    match value {
        Value::String(text) if text.starts_with('$') => {
            captured.get(&text[1..]).cloned().unwrap_or(Value::Null)
        }
        Value::Array(items) => items
            .iter()
            .map(|item| substitute(item, captured))
            .collect(),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, item)| (key.clone(), substitute(item, captured)))
            .collect(),
        other => other.clone(),
    }
}

/// risk: F8 (mounted from rutis.json, a Cordis plugin behaves otherwise
/// than mounted by a library, or than in Cordis)
///
/// guarantee: loader/announced-services, host/check
#[test]
fn published_cordis_services_resolve_and_answer_as_in_native_cordis() {
    let s = Scenario::new("cordis-services");
    // Native Cordis first, in the directories both runs share (file
    // versions compare equal): `node native.mjs <root>`.
    let root = s.dir().join("baseline");
    let root_text = root.display().to_string();
    let native = node(&s, "native.mjs", &[&root_text]);
    let targets = node(&s, "scenarios.mjs", &[&root_text]);
    let targets = targets.as_array().expect("the scenarios");
    assert!(!targets.is_empty(), "scenarios.mjs gives no target");

    // Every plugin of every target a row; a TS probe in the same Node
    // runtime uses every target's service.
    node_project(&s);
    s.link_node_sdk();
    let services: Vec<&str> = targets
        .iter()
        .map(|target| target["service"].as_str().unwrap())
        .collect();
    let probe = s.probe("probe", Lang::Node, &services);
    let mut rows = Vec::new();
    for target in targets {
        for (index, plugin) in target["plugins"].as_array().unwrap().iter().enumerate() {
            rows.push(json!({
                "id": format!("{}-{index}", target["name"].as_str().unwrap()),
                "name": plugin["package"],
                "config": plugin["config"],
            }));
        }
    }
    rows.push(probe.row());
    s.write_json(
        "rutis.json",
        &json!({ "runtimes": { "node": {} }, "rows": rows }),
    );

    // `check` passes, warning: what the rows inject, other rows of the
    // runtime may provide once they run (nothing declares it), and in
    // Cordis it resolves natively.
    let mut check = s.host(["check", "rutis.json"]);
    let status = check.wait_exit();
    let printed = output(&check).join("\n");
    assert!(status.success(), "check: {status}\n{printed}");
    assert!(
        !printed.contains("nothing provides"),
        "check, dependencies between rows:\n{printed}"
    );
    assert!(
        printed.contains(
            "rutis-host: warning: workspace-4: nothing declares storageDomain; it may be provided at runtime by "
        ),
        "check, what workspace uses:\n{printed}"
    );

    let mut host = s.host(["run", "rutis.json"]);
    probe.started(&mut host);
    for target in targets {
        let name = target["name"].as_str().unwrap();
        let service = target["service"].as_str().unwrap();
        let calls: Vec<Call> = target["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| Call {
                method: call["method"].as_str().unwrap().to_owned(),
                args: call["args"].clone(),
                capture: call["capture"].as_str().map(str::to_owned),
            })
            .collect();
        assert!(!calls.is_empty(), "target {name} makes no call");
        let mut captured = Map::new();
        let mut results = Vec::new();
        for call in calls {
            let args = substitute(&call.args, &captured);
            let result = probe.call(&mut host, service, &call.method, args);
            if let (Some(capture), Ok(value)) = (&call.capture, &result) {
                captured.insert(capture.clone(), value.clone());
            }
            // Compared as the probe reports them: a value, or a message.
            results.push(match result {
                Ok(value) => json!({ "ok": value }),
                Err(message) => json!({ "error": message }),
            });
        }
        let expected: Vec<Value> = native[name]["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|result| match result.get("error") {
                Some(error) => json!({ "error": error["message"] }),
                None => json!({ "ok": result["ok"] }),
            })
            .collect();
        assert_eq!(results, expected, "target {name}");
    }

    host.ctrl_c();
    let status = host.wait_exit();
    assert_eq!(status.code(), Some(0), "after Ctrl-C: {status}");
    s.finish();
}
