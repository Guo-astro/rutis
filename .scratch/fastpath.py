import json

d = json.load(open("/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37/crates/rutis-bridge/src/runtime/testing/events.expected.json"))
for c in d["cases"]:
    text = json.dumps(c)
    if "fast-path" in c["case"] or "viaHost" in text:
        print(c["case"], {k: v for k, v in c.items() if k in ("only", "except", "tool", "rule", "where", "placements")}, "viaHost" in text)
for x in d["about"]:
    if "only" in x and "except" in x:
        print(">>", x[:700])
