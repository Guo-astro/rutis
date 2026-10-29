import pathlib

ROOT = pathlib.Path("/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37")


def edit(path, pairs):
    p = ROOT / path
    s = p.read_text()
    for old, new in pairs:
        assert s.count(old) == 1, (path, old[:90], s.count(old))
        s = s.replace(old, new)
    p.write_text(s)


for lang, sep in (("md", "、"), ("en.md", ", ")):
    path = f"docs/guarantees.{lang}"
    edit(path, [
        # events/one-bus and sdk/leaf-events: the Python testing tool no longer runs the cases.
        (f"`python/rutis/tests/test_events_conformance.py::EventExpectations::test_the_testing_tool_meets_the_event_expectations`{sep}`go/rutis/rutistest/events_conformance_test.go::TestRutistestMeetsTheEventExpectations`{sep}`bun/rutis-bun/test/events-conformance.test.ts` |",
         f"`go/rutis/rutistest/events_conformance_test.go::TestRutistestMeetsTheEventExpectations`{sep}`bun/rutis-bun/test/events-conformance.test.ts` |"),
        (f"`python/rutis/tests/test_events_conformance.py::EventExpectations::test_the_testing_tool_meets_the_event_expectations`{sep}`go/rutis/rutistest/events_conformance_test.go::TestRutistestMeetsTheEventExpectations`{sep}`bun/rutis-bun/test/events-conformance.test.ts`{sep}`node/rutis-runtime/test/leaf.test.mjs`",
         f"`go/rutis/rutistest/events_conformance_test.go::TestRutistestMeetsTheEventExpectations`{sep}`bun/rutis-bun/test/events-conformance.test.ts`{sep}`node/rutis-runtime/test/leaf.test.mjs`"),
        (f"`python/rutis/tests/test_events.py::Unload::test_unload_cancels_running_listeners_and_waits_for_them`{sep}`::Unload::test_listeners_leave_on_update_and_failed_apply`{sep}`python/rutis/tests/test_events_session.py::Handshake::test_mount_has_no_events_feature`{sep}`python/rutis/tests/test_events.py::Scope::test_events_are_one_flat_bus`{sep}`python/rutis/tests/test_testing.py::Testing::test_plugins_on_one_bus_hear_each_other_and_filter`{sep}`::test_event_disposers_and_failed_cleanup_remove_every_listener`{sep}`::test_unload_cancels_unfinished_local_listeners`",
         f"`python/rutis/tests/test_events_session.py::Handshake::test_mount_has_no_events_feature`{sep}`::CalledByTheHost::test_unload_cancels_a_listener_the_host_is_running`{sep}`python/rutis/tests/test_testing.py::Testing::test_plugins_on_one_bus_hear_each_other_and_filter`{sep}`::test_an_unloaded_row_emits_nothing`{sep}`::test_unload_cancels_unfinished_listeners`"),
        # sdk/testing-matches-runtime: the Python tool is the real runtime now.
        (f"`node/rutis/test/conformance.test.mjs`{sep}`python/rutis/tests/test_conformance.py::Conformance::test_the_testing_tool_returns_what_the_real_runtime_does` |",
         "`node/rutis/test/conformance.test.mjs` |"),
    ])

edit("docs/guarantees.md", [
    ("`python/rutis/tests/test_config.py`（`Cases`、`TestingTool`、`RuntimeRows` 的 `test_every_case`）、", ""),
    ("测试工具（`@arcships/rutis/testing`、`rutis.testing`，严格模式）对插件的调用结果与真实运行时相同", "测试工具（`@arcships/rutis/testing`，严格模式）对插件的调用结果与真实运行时相同"),
    ("上面各个配置测试；插件值：", "上面各个配置测试、`python/rutis/tests/test_testing.py::Testing::test_the_hosts_loader_checks_the_config_and_fills_its_defaults`；插件值："),
])
edit("docs/guarantees.en.md", [
    ("`python/rutis/tests/test_config.py` (`test_every_case` of `Cases`, `TestingTool` and `RuntimeRows`), ", ""),
    ("The test tools (`@arcships/rutis/testing`, `rutis.testing`, strict mode) give the same results", "The test tools (`@arcships/rutis/testing`, strict mode) give the same results"),
    ("the config tests above; plugin values:", "the config tests above, `python/rutis/tests/test_testing.py::Testing::test_the_hosts_loader_checks_the_config_and_fills_its_defaults`; plugin values:"),
])
edit("docs/faults.md", [
    ("- Python：`python/rutis/tests/test_config.py` `test_a_bad_config_crosses_with_its_code_and_issues`，`python/rutis/tests/test_config.py` `test_an_extra_key_is_warned_about_once`",
     "- Python：`python/rutis/tests/test_testing.py` `test_the_hosts_loader_checks_the_config_and_fills_its_defaults`"),
])
edit("docs/faults.en.md", [
    ("- Python: `python/rutis/tests/test_config.py` `test_a_bad_config_crosses_with_its_code_and_issues`; `python/rutis/tests/test_config.py` `test_an_extra_key_is_warned_about_once`",
     "- Python: `python/rutis/tests/test_testing.py` `test_the_hosts_loader_checks_the_config_and_fills_its_defaults`"),
])
print("ok")
