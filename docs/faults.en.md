# Fault acceptance summary

[中文](faults.md) · [Release process](release.en.md) · [CI](ci.en.md)

The debugging evaluation of [#663](https://github.com/arcships/rutis/issues/663) causes 14 faults, one at a time, in each language and looks at what developers see. These faults get no test suite of their own ([#690](https://github.com/arcships/rutis/issues/690)): the acceptance tests of [#677](https://github.com/arcships/rutis/issues/677), [#678](https://github.com/arcships/rutis/issues/678), [#676](https://github.com/arcships/rutis/issues/676), [#669](https://github.com/arcships/rutis/issues/669) and others already cause each fault, check the output the developer sees (row name, service.method, cause and fix, no words internal to rutis), and check that the fault does not spread to other plugins. The table below lists the tests that cover each fault × each language.

## Where they run

- Every test in the table is in a job of `ci.yml`: the tests of rutis, rutis-host, rutis-loader and rutis-bridge in `rust`, the end-to-end scenarios in `e2e`, the Node and Python packages in `js-py`, Bun in `runtimes-bun`, Go in `runtimes-go`. The release gate (release.yml) and the nightly run (nightly.yml) both call `ci.yml` with `all=true`, so when one of them fails in a release, the gate fails and nothing is published; a nightly failure opens an issue ([CI](ci.en.md) §7).
- None skips quietly: the Go Delve test runs in the `rust` job with `RUTIS_TEST_DELVE=required`, so a missing `dlv` fails it; the event conformance tests run with `RUTIS_EVENTS_REQUIRE=node,python,go,bun`, so a missing language fails them; the Unix-only tests run on Linux.
- The `links` job runs `node tools/check-faults.mjs`: it fails when a test the table lists is renamed or removed, or this page differs from `tests/faults.json`; it also writes the table into the run's summary.

## Updating it

The table comes from `tests/faults.json`. When an acceptance test changes (renamed, added, or filling a "no test" or "pending" cell), change that file and run `node tools/check-faults.mjs --write` to regenerate both language copies of this page. A cell says:

- "n tests": n tests cover it;
- "pending #N": part of this fault is covered by #N, not merged yet; added to the table once it is;
- "no test": no test yet, one is to be added;
- "n/a": the language has no such fault (a Rust plugin runs in the host's process, with no runtime process of its own to exit).

## The table

<!-- faults:begin (node tools/check-faults.mjs --write) -->

| # | Fault | TypeScript (Node) | Bun | Python | Go | Rust (plugin and embedding) | Issues |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | A dependency's service name misspelt | 3 tests | 1 test | 1 test | 2 tests | 3 tests | [#676](https://github.com/arcships/rutis/issues/676) [#677](https://github.com/arcships/rutis/issues/677) |
| 2 | A service method throws, across languages | 3 tests | 2 tests | 3 tests | 2 tests | 1 test | [#677](https://github.com/arcships/rutis/issues/677) |
| 3 | No such method, wrong number or type of arguments | 2 tests | 2 tests | 3 tests | 2 tests | 2 tests | [#677](https://github.com/arcships/rutis/issues/677) |
| 4 | An error in `apply` | 3 tests | 2 tests | 1 test | 1 test | 2 tests | [#677](https://github.com/arcships/rutis/issues/677) |
| 5 | A syntax, import or compile error | 2 tests | 3 tests | 3 tests | 1 test | 1 test | [#677](https://github.com/arcships/rutis/issues/677) |
| 6 | A value passed across languages that the other side cannot take | 3 tests | 3 tests | 3 tests | 2 tests | 2 tests | [#675](https://github.com/arcships/rutis/issues/675) [#687](https://github.com/arcships/rutis/issues/687) |
| 7 | An event listener throws | 1 test | 1 test | 1 test | 1 test | 2 tests | [#677](https://github.com/arcships/rutis/issues/677) |
| 8 | A call that never returns | 4 tests | 3 tests | 6 tests | no test | 2 tests | [#678](https://github.com/arcships/rutis/issues/678) |
| 9 | Plugins calling or depending on each other | 2 tests | 1 test | 2 tests | 1 test | 2 tests | [#676](https://github.com/arcships/rutis/issues/676) [#228](https://github.com/arcships/rutis/issues/228) |
| 10 | A runtime process exits midway | 2 tests | 2 tests | 4 tests | 2 tests | n/a | [#682](https://github.com/arcships/rutis/issues/682) [#688](https://github.com/arcships/rutis/issues/688) |
| 11 | Breakpoints and logs | 4 tests | 2 tests | 3 tests | 1 test | 1 test | [#679](https://github.com/arcships/rutis/issues/679) [#678](https://github.com/arcships/rutis/issues/678) |
| 12 | A wrong config | 3 tests | 2 tests | 2 tests | 3 tests | 3 tests | [#669](https://github.com/arcships/rutis/issues/669) |
| 13 | Two plugins provide a service of the same name | 3 tests | 2 tests | 2 tests | 1 test | 2 tests | [#676](https://github.com/arcships/rutis/issues/676) [#677](https://github.com/arcships/rutis/issues/677) |
| 14 | Seeing what is running now | 1 test | no test | 1 test | no test | 2 tests | [#678](https://github.com/arcships/rutis/issues/678) |

### 1. A dependency's service name misspelt

- TypeScript (Node): `crates/rutis-host/tests/waiting.rs` `a_waiting_row_says_what_it_waits_for_and_why`; `crates/rutis-host/tests/check.rs` `check_fails_on_cycles_and_clashes_and_suggests_misspelt_names`; `node/rutis-runtime/test/leaf.test.mjs` “a leaf plugin using a service it does not inject is told to declare it”
- Bun: `bun/rutis-bun/test/runner.test.ts` “a plugin using a service it does not inject is told to declare it”
- Python: `python/rutis/tests/test_undeclared_use.py` `test_a_service_not_in_inject_is_named_with_the_fix`
- Go: `go/rutis/sdk_test.go` `TestAPluginMayUseOnlyWhatItInjects`; `go/rutis/internal/runtime/errors_text_internal_test.go` `TestUseSaysWhyAServiceIsNotThere`
- Rust (plugin and embedding): `crates/rutis-loader/tests/waits.rs` `a_misspelt_name_is_waited_for_with_the_name_it_meant`; `crates/rutis-loader/tests/check.rs` `a_disabled_provider_is_named_and_a_misspelt_name_corrected`; `crates/rutis/tests/still_waiting.rs` `a_plugin_waiting_long_for_its_deps_is_reported_once`

### 2. A service method throws, across languages

- TypeScript (Node): `crates/rutis-loader/tests/fault_texts.rs` `fault_messages_hold_none_of_the_platforms_words`; `crates/rutis-loader/tests/runtime_rows.rs` `an_error_from_a_service_method_says_where_it_came_from`; `node/rutis-runtime/test/errors.test.mjs` “an error from the other side thrown on adds where it was thrown on to its root (#677)”
- Bun: `bun/rutis-bun/test/errors.test.ts` “an error from the other side thrown on adds where it was thrown on to its root (#677)”; `bun/rutis-bun/test/bad-values.test.ts` “an error answering a call has the caller's stack, and a rethrow adds it (#677)”
- Python: `crates/rutis-loader/tests/fault_texts.rs` `fault_messages_hold_none_of_the_platforms_words`; `python/rutis/tests/test_peer.py` `test_a_remote_error_thrown_on_crosses_with_its_whole_graph`; `python/rutis/tests/test_plain_texts.py` `test_a_traceback_through_a_host_service_names_the_service_and_method`
- Go: `go/rutis/internal/runtime/errors_location_test.go` `TestAServiceErrorCrossesWithItsMethodsPosition`; `go/rutis/internal/runtime/errors_location_test.go` `TestAReceivedErrorThrownOnRecordsTheMethodInRethrown`
- Rust (plugin and embedding): `crates/rutis-bridge/tests/marked_services.rs` `a_provided_marked_type_is_served_by_name`

### 3. No such method, wrong number or type of arguments

- TypeScript (Node): `crates/rutis-loader/tests/fault_texts.rs` `fault_messages_hold_none_of_the_platforms_words`; `node/rutis-runtime/test/texts.test.mjs` “a row that cannot load, or is refused a call, is told in the developer's words”
- Bun: `bun/rutis-bun/test/errors.test.ts` “the errors this runtime makes have their code, data and text (#677)”; `bun/rutis-bun/test/bad-values.test.ts` “a method an object passed by reference lacks is no-method, saying what it has”
- Python: `python/rutis/tests/test_plain_texts.py` `test_no_method`; `python/rutis/tests/test_plain_texts.py` `test_too_many_arguments`; `python/rutis/tests/test_plain_texts.py` `test_a_missing_argument`
- Go: `crates/rutis-loader/tests/fault_texts.rs` `fault_messages_hold_none_of_the_platforms_words`; `go/rutis/internal/runtime/errors_text_internal_test.go` `TestARefusedCallNamesTheServiceMethodAndArgument`
- Rust (plugin and embedding): `crates/rutis-loader/tests/fault_texts.rs` `fault_messages_hold_none_of_the_platforms_words`; `crates/rutis-bridge/src/marked.rs` `wrong_calls_name_the_service_method_and_argument`

### 4. An error in `apply`

- TypeScript (Node): `crates/rutis-host/tests/dev.rs` `dev_follows_a_node_project`; `crates/rutis-loader/tests/runtime_rows.rs` `a_failed_row_keeps_the_plugins_error`; `node/rutis-runtime/test/errors.test.mjs` “an apply failure crosses with the plugin's frames only (#677)”
- Bun: `crates/rutis-host/tests/dev.rs` `dev_follows_a_bun_project`; `bun/rutis-bun/test/runner.test.ts` “an apply failure crosses with the plugin's frames only (#677)”
- Python: `crates/rutis-host/tests/dev.rs` `dev_follows_a_python_project`
- Go: `crates/rutis-host/tests/dev.rs` `dev_rebuilds_a_go_project_only_for_its_sources`
- Rust (plugin and embedding): `crates/rutis/tests/embedding_output.rs` `a_failed_start_reaches_the_error_sink_by_the_plugins_name`; `crates/rutis-loader/tests/waits.rs` `a_row_that_cannot_start_reaches_the_error_sink_by_its_name`

### 5. A syntax, import or compile error

- TypeScript (Node): `tests/e2e/tests/dev_loop.rs` `the_node_plugin_development_loop`; `crates/rutis-host/tests/check.rs` `check_fails_what_cannot_run`
- Bun: `tests/e2e/tests/dev_loop.rs` `the_bun_plugin_development_loop`; `bun/rutis-bun/test/plugin.test.ts` “a module that does not parse fails with its file and line”; `bun/rutis-bun/test/plugin.test.ts` “an import error names the submodule that threw, and a gone entry is said so (#677)”
- Python: `tests/e2e/tests/dev_loop.rs` `the_python_plugin_development_loop`; `python/rutis/tests/test_plain_texts.py` `test_a_syntax_error_says_its_file_and_line`; `python/rutis/tests/test_plain_texts.py` `test_the_submodule_that_raised_is_named`
- Go: `tests/e2e/tests/dev_loop.rs` `the_go_plugin_development_loop`
- Rust (plugin and embedding): `crates/rutis/tests/compile_errors.rs` `author_mistakes_are_reported_in_the_authors_terms`

### 6. A value passed across languages that the other side cannot take

- TypeScript (Node): `crates/rutis-loader/tests/bad_values.rs` `a_bad_value_fails_only_its_call_between_node_python_and_go`; `crates/rutis-bridge/tests/value_roundtrip.rs` `node_bad_values`; `crates/rutis-loader/tests/objects_cross.rs` `a_node_object_crosses_to_python`
- Bun: `crates/rutis-bridge/tests/value_roundtrip.rs` `bun_bad_values`; `crates/rutis-loader/tests/objects_cross.rs` `a_bun_object_crosses_to_node`; `bun/rutis-bun/test/bad-values.test.ts` “a value that cannot be sent carries code bad-value and its part (#677)”
- Python: `crates/rutis-loader/tests/bad_values.rs` `a_bad_value_fails_only_its_call_between_node_python_and_go`; `crates/rutis-bridge/tests/value_roundtrip.rs` `python_bad_values`; `crates/rutis-loader/tests/objects_cross.rs` `a_python_object_crosses_to_node`
- Go: `crates/rutis-loader/tests/bad_values.rs` `a_bad_value_fails_only_its_call_between_node_python_and_go`; `crates/rutis-loader/tests/objects_cross.rs` `a_go_parameter_that_takes_no_object_says_so`
- Rust (plugin and embedding): `crates/rutis-bridge/src/session/rpc/tests/objects.rs` `a_rust_service_refuses_an_object_argument`; `crates/rutis-bridge/tests/value_roundtrip.rs` `rust_relays_a_thrown_value`

### 7. An event listener throws

- TypeScript (Node): `crates/rutis-loader/tests/events_conformance.rs` `events_with_x_a_node_leaf_plugin`
- Bun: `crates/rutis-loader/tests/events_conformance.rs` `events_with_x_a_bun_plugin`
- Python: `crates/rutis-loader/tests/events_conformance.rs` `events_with_x_a_python_plugin`
- Go: `crates/rutis-loader/tests/events_conformance.rs` `events_with_x_a_go_plugin`
- Rust (plugin and embedding): `crates/rutis-loader/tests/events_conformance.rs` `events_with_x_a_node_leaf_plugin`; `crates/rutis-bridge/src/events/errors.rs` `a_listener_error_records_its_row_and_event`

### 8. A call that never returns

- TypeScript (Node): `crates/rutis-host/tests/status.rs` `status_says_what_a_running_host_does`; `node/rutis-runtime/test/output.test.mjs` “a method holding the runtime is said once; a short one is not”; `node/rutis-runtime/test/output.test.mjs` “a method awaiting for long does not hold the runtime, and is not said”; `node/rutis-runtime/test/output.test.mjs` “under a debugger nothing is said of a method holding the runtime”
- Bun: `bun/rutis-bun/test/output.test.ts` “a method holding the runtime is said once; a short one is not”; `bun/rutis-bun/test/output.test.ts` “a method awaiting for long does not hold the runtime, and is not said”; `bun/rutis-bun/test/output.test.ts` “under a debugger nothing is said of a method holding the runtime”
- Python: `crates/rutis-host/tests/status.rs` `status_says_what_a_running_host_does`; `crates/rutis-loader/tests/status.rs` `the_status_names_rows_services_runtimes_and_calls`; `python/rutis/tests/test_output.py` `test_a_method_holding_the_runtime_is_said_once`; `python/rutis/tests/test_output.py` `test_a_blocking_call_in_an_async_method_is_said`; `python/rutis/tests/test_output.py` `test_an_async_method_awaiting_long_is_not_said`; `python/rutis/tests/test_output.py` `test_under_a_debugger_nothing_is_said`
- Go: no test
- Rust (plugin and embedding): `crates/rutis/tests/embedding_output.rs` `an_apply_that_does_not_return_is_named`; `crates/rutis/tests/embedding_output.rs` `a_shutdown_that_runs_out_of_time_names_the_stuck_cleanup`

### 9. Plugins calling or depending on each other

- TypeScript (Node): `crates/rutis-host/tests/check.rs` `check_fails_on_cycles_and_clashes_and_suggests_misspelt_names`; `crates/rutis-loader/tests/sync_cycles.rs` `two_node_runtimes_calling_each_other`
- Bun: `crates/rutis-loader/tests/sync_cycles.rs` `an_emit_listener_run_while_a_call_waits`
- Python: `crates/rutis-loader/tests/sync_cycles.rs` `one_node_and_two_python_runtimes`; `python/rutis/tests/test_plain_texts.py` `test_a_sync_wait_cycle_prints_its_message_alone`
- Go: `crates/rutis-loader/tests/sync_cycles.rs` `two_node_runtimes_waiting_on_each_other_through_go`
- Rust (plugin and embedding): `crates/rutis-loader/tests/waits.rs` `rows_that_inject_each_other_say_so`; `crates/rutis-loader/tests/check.rs` `rows_that_inject_each_other_are_a_cycle`

### 10. A runtime process exits midway

- TypeScript (Node): `crates/rutis-loader/tests/fault_containment.rs` `node_a_runtime_that_exits_blames_no_other_row`; `crates/rutis-host/tests/unhandled.rs` `a_plugin_ending_its_process_is_named`
- Bun: `crates/rutis-loader/tests/fault_containment.rs` `bun_a_runtime_that_exits_blames_no_other_row`; `crates/rutis-host/tests/unhandled.rs` `a_plugin_ending_its_process_is_named`
- Python: `crates/rutis-loader/tests/fault_containment.rs` `python_a_runtime_that_exits_blames_no_other_row`; `crates/rutis-host/tests/runtimes.rs` `a_killed_runtime_is_reported_with_how_it_ended`; `tests/e2e/tests/runtime_crash.rs` `a_killed_runtime_fails_its_calls_and_a_restart_recovers`; `crates/rutis-host/tests/unhandled.rs` `a_plugin_ending_its_process_is_named`
- Go: `crates/rutis-loader/tests/go_rows.rs` `a_crash_stops_only_its_runtimes_rows_and_is_not_restarted`; `crates/rutis-host/tests/runtimes.rs` `a_go_runtime_restarted_on_failure_brings_its_rows_back`
- Rust (plugin and embedding): n/a

### 11. Breakpoints and logs

- TypeScript (Node): `crates/rutis-host/tests/debug.rs` `dev_debug_opens_the_node_inspector`; `crates/rutis-host/tests/output.rs` `plugin_lines_written_in_pieces_come_out_whole`; `node/rutis-runtime/test/output.test.mjs` “each line a row's code writes starts with the row's name”; `crates/rutis-host/tests/output.rs` `prefix_output_starts_each_plugin_line_with_its_row`
- Bun: `crates/rutis-host/tests/debug.rs` `dev_debug_opens_the_bun_inspector`; `bun/rutis-bun/test/output.test.ts` “each line a row's code writes starts with the row's name”
- Python: `crates/rutis-host/tests/debug.rs` `dev_debug_stops_python_at_breakpoint`; `python/rutis/tests/test_output.py` `test_each_line_a_row_writes_starts_with_its_name`; `crates/rutis-host/tests/output.rs` `prefix_output_starts_each_plugin_line_with_its_row`
- Go: `crates/rutis-host/tests/debug.rs` `dev_debug_runs_the_binary_under_delve`
- Rust (plugin and embedding): `crates/rutis/src/error.rs` `default_sink_lines_start_with_the_name_they_are_about`

### 12. A wrong config

- TypeScript (Node): `crates/rutis-host/tests/check.rs` `check_fails_a_config_that_does_not_match_the_schema`; `crates/rutis-host/tests/check.rs` `check_warns_of_extra_config_keys`; `node/rutis-runtime/test/config.test.mjs` “a leaf plugin gets its config filled, and fails its row on a wrong one”
- Bun: `bun/rutis-bun/test/config.test.ts` “a row's config is filled, and a wrong one fails the row with bad-config”; `bun/rutis-bun/test/config.test.ts` “this copy passes every case of the shared list”
- Python: `python/rutis/tests/test_config.py` `test_a_bad_config_crosses_with_its_code_and_issues`; `python/rutis/tests/test_config.py` `test_an_extra_key_is_warned_about_once`
- Go: `go/rutis/internal/runtime/config_internal_test.go` `TestATypedConfigIsDecoded`; `go/rutis/internal/runtime/config_internal_test.go` `TestAConfigTheTypeDoesNotTakeIsSaidPlainly`; `go/rutis/rutistest/config_test.go` `TestATypedConfigInTheTestTool`
- Rust (plugin and embedding): `crates/rutis-loader/tests/config_rows.rs` `a_rust_row_is_filled_and_validated`; `crates/rutis-loader/tests/config_rows.rs` `extra_keys_are_warned_about_once`; `crates/rutis/tests/config_cases.rs` `every_case_of_the_shared_list_passes`

### 13. Two plugins provide a service of the same name

- TypeScript (Node): `crates/rutis-host/tests/clash.rs` `a_service_provided_after_loading_twice_fails_the_later_row`; `crates/rutis-loader/tests/fault_containment.rs` `node_a_clashing_provide_fails_only_its_row`; `crates/rutis-host/tests/check.rs` `check_fails_on_cycles_and_clashes_and_suggests_misspelt_names`
- Bun: `crates/rutis-loader/tests/fault_containment.rs` `bun_a_clashing_provide_fails_only_its_row`; `bun/rutis-bun/test/runner.test.ts` “a row that cannot load or provides a taken service is told so plainly (#677)”
- Python: `crates/rutis-loader/tests/fault_containment.rs` `python_a_clashing_provide_fails_only_its_row`; `python/rutis/tests/test_plain_texts.py` `test_a_service_provided_twice_names_the_row`
- Go: `go/rutis/internal/runtime/errors_text_internal_test.go` `TestADuplicateProviderIsNamedByItsRow`
- Rust (plugin and embedding): `crates/rutis-loader/tests/check.rs` `rows_declaring_one_service_in_one_scope_clash`; `crates/rutis-bridge/tests/marked_services.rs` `a_second_service_of_the_same_name_is_refused`

### 14. Seeing what is running now

- TypeScript (Node): `crates/rutis-host/tests/status.rs` `status_says_what_a_running_host_does`
- Bun: no test
- Python: `crates/rutis-loader/tests/status.rs` `the_status_names_rows_services_runtimes_and_calls`
- Go: no test
- Rust (plugin and embedding): `crates/rutis/tests/embedding_output.rs` `diagnostics_print_by_name_without_rutis_own_plugins`; `crates/rutis-loader/tests/status.rs` `the_status_names_rows_services_runtimes_and_calls`

<!-- faults:end -->
