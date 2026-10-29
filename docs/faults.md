# 故障验收汇总

[English](faults.en.md) · [发布流程](release.md) · [CI](ci.md)

[#663](https://github.com/arcships/rutis/issues/663) 的调试评估在每种语言里逐种制造 14 种故障，看开发者看到什么。这些故障不另写一套测试（[#690](https://github.com/arcships/rutis/issues/690)）：[#677](https://github.com/arcships/rutis/issues/677)、[#678](https://github.com/arcships/rutis/issues/678)、[#676](https://github.com/arcships/rutis/issues/676)、[#669](https://github.com/arcships/rutis/issues/669) 等各自的验收测试已经制造这种故障，比较开发者看到的输出（行名、服务.方法、原因和改法、没有 rutis 内部的词），并确认故障不波及别的插件。下表列出每种故障 × 每种语言由哪些测试负责。

## 在哪里运行

- 表里的测试都在 `ci.yml` 的任务里运行：Rust、rutis-host、rutis-loader、rutis-bridge 的测试在 `rust`，端到端场景在 `e2e`，Node 与 Python 的包在 `js-py`，Bun 在 `runtimes-bun`，Go 在 `runtimes-go`。发布门（release.yml）和每晚运行（nightly.yml）都以 `all=true` 调用 `ci.yml`，所以发布时其中任何一个失败，发布门就不通过，什么都不发布；每晚运行失败时开问题单（[CI](ci.md) §7）。
- 不会悄悄跳过：Go 的 Delve 测试在 `rust` 任务里设了 `RUTIS_TEST_DELVE=required`，没装 `dlv` 就失败；事件一致性测试设了 `RUTIS_EVENTS_REQUIRE=node,python,go,bun`，少了哪种语言就失败；只在 Unix 上运行的测试在 Linux 上运行。
- `links` 任务运行 `node tools/check-faults.mjs`：表里列的测试改了名或删掉了、或者本页与 `tests/faults.json` 不一致，就失败；它还把下表写进这次运行的摘要。

## 怎样更新

表来自 `tests/faults.json`。改了验收测试（改名、新增、补上「无测试」或「待接」的格子）时改这个文件，再运行 `node tools/check-faults.mjs --write` 重新生成中英两份。格子里写：

- 「n 个测试」：有 n 个测试负责；
- 「待接 #N」：这种故障的一部分由还没合并的 #N 负责，合并后补进表里；
- 「无测试」：还没有测试，要补；
- 「不适用」：这种语言没有这种故障（Rust 插件和宿主在同一个进程里，没有单独退出的运行时进程）。

## 表

<!-- faults:begin (node tools/check-faults.mjs --write) -->

| # | 故障 | TypeScript（Node） | Bun | Python | Go | Rust（插件与嵌入） | issue |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 依赖的服务名写错 | 3 个测试 | 1 个测试 | 1 个测试 | 2 个测试 | 3 个测试 | [#676](https://github.com/arcships/rutis/issues/676) [#677](https://github.com/arcships/rutis/issues/677) |
| 2 | 服务方法抛错，跨语言 | 3 个测试 | 2 个测试 | 3 个测试 | 2 个测试 | 1 个测试 | [#677](https://github.com/arcships/rutis/issues/677) |
| 3 | 方法不存在、参数个数或类型错 | 2 个测试 | 2 个测试 | 3 个测试 | 2 个测试 | 2 个测试 | [#677](https://github.com/arcships/rutis/issues/677) |
| 4 | `apply` 里出错 | 3 个测试 | 2 个测试 | 1 个测试 | 1 个测试 | 2 个测试 | [#677](https://github.com/arcships/rutis/issues/677) |
| 5 | 语法 / 导入 / 编译错误 | 2 个测试 | 3 个测试 | 3 个测试 | 1 个测试 | 1 个测试 | [#677](https://github.com/arcships/rutis/issues/677) |
| 6 | 跨语言传对面接不住的值 | 3 个测试 | 3 个测试 | 3 个测试 | 2 个测试 | 2 个测试 | [#675](https://github.com/arcships/rutis/issues/675) [#687](https://github.com/arcships/rutis/issues/687) |
| 7 | 事件监听器出错 | 1 个测试 | 1 个测试 | 1 个测试 | 1 个测试 | 2 个测试 | [#677](https://github.com/arcships/rutis/issues/677) |
| 8 | 调用一直不返回 | 4 个测试 | 3 个测试 | 6 个测试 | 无测试 | 2 个测试 | [#678](https://github.com/arcships/rutis/issues/678) |
| 9 | 互相调用、互相依赖 | 2 个测试 | 1 个测试 | 2 个测试 | 1 个测试 | 2 个测试 | [#676](https://github.com/arcships/rutis/issues/676) [#228](https://github.com/arcships/rutis/issues/228) |
| 10 | 运行时进程中途退出 | 2 个测试 | 2 个测试 | 4 个测试 | 2 个测试 | 不适用 | [#682](https://github.com/arcships/rutis/issues/682) [#688](https://github.com/arcships/rutis/issues/688) |
| 11 | 下断点、看日志 | 4 个测试 | 2 个测试 | 3 个测试 | 1 个测试 | 1 个测试 | [#679](https://github.com/arcships/rutis/issues/679) [#678](https://github.com/arcships/rutis/issues/678) |
| 12 | 配置写错 | 3 个测试 | 1 个测试 | 2 个测试 | 3 个测试 | 3 个测试 | [#669](https://github.com/arcships/rutis/issues/669) |
| 13 | 两个插件提供同名服务 | 3 个测试 | 2 个测试 | 2 个测试 | 1 个测试 | 2 个测试 | [#676](https://github.com/arcships/rutis/issues/676) [#677](https://github.com/arcships/rutis/issues/677) |
| 14 | 查看现在的运行状态 | 1 个测试 | 无测试 | 1 个测试 | 无测试 | 2 个测试 | [#678](https://github.com/arcships/rutis/issues/678) |

### 1. 依赖的服务名写错

- TypeScript（Node）：`crates/rutis-host/tests/waiting.rs` `a_waiting_row_says_what_it_waits_for_and_why`，`crates/rutis-host/tests/check.rs` `check_fails_on_cycles_and_clashes_and_suggests_misspelt_names`，`node/rutis-runtime/test/leaf.test.mjs` “a leaf plugin using a service it does not inject is told to declare it”
- Bun：`bun/rutis-bun/test/runner.test.ts` “a plugin using a service it does not inject is told to declare it”
- Python：`python/rutis/tests/test_undeclared_use.py` `test_a_service_not_in_inject_is_named_with_the_fix`
- Go：`go/rutis/sdk_test.go` `TestAPluginMayUseOnlyWhatItInjects`，`go/rutis/internal/runtime/errors_text_internal_test.go` `TestUseSaysWhyAServiceIsNotThere`
- Rust（插件与嵌入）：`crates/rutis-loader/tests/waits.rs` `a_misspelt_name_is_waited_for_with_the_name_it_meant`，`crates/rutis-loader/tests/check.rs` `a_disabled_provider_is_named_and_a_misspelt_name_corrected`，`crates/rutis/tests/still_waiting.rs` `a_plugin_waiting_long_for_its_deps_is_reported_once`

### 2. 服务方法抛错，跨语言

- TypeScript（Node）：`crates/rutis-loader/tests/fault_texts.rs` `fault_messages_hold_none_of_the_platforms_words`，`crates/rutis-loader/tests/runtime_rows.rs` `an_error_from_a_service_method_says_where_it_came_from`，`node/rutis-runtime/test/errors.test.mjs` “an error from the other side thrown on adds where it was thrown on to its root (#677)”
- Bun：`bun/rutis-bun/test/errors.test.ts` “an error from the other side thrown on adds where it was thrown on to its root (#677)”，`bun/rutis-bun/test/bad-values.test.ts` “an error answering a call has the caller's stack, and a rethrow adds it (#677)”
- Python：`crates/rutis-loader/tests/fault_texts.rs` `fault_messages_hold_none_of_the_platforms_words`，`python/rutis/tests/test_peer.py` `test_a_remote_error_thrown_on_crosses_with_its_whole_graph`，`python/rutis/tests/test_plain_texts.py` `test_a_traceback_through_a_host_service_names_the_service_and_method`
- Go：`go/rutis/internal/runtime/errors_location_test.go` `TestAServiceErrorCrossesWithItsMethodsPosition`，`go/rutis/internal/runtime/errors_location_test.go` `TestAReceivedErrorThrownOnRecordsTheMethodInRethrown`
- Rust（插件与嵌入）：`crates/rutis-bridge/tests/marked_services.rs` `a_provided_marked_type_is_served_by_name`

### 3. 方法不存在、参数个数或类型错

- TypeScript（Node）：`crates/rutis-loader/tests/fault_texts.rs` `fault_messages_hold_none_of_the_platforms_words`，`node/rutis-runtime/test/texts.test.mjs` “a row that cannot load, or is refused a call, is told in the developer's words”
- Bun：`bun/rutis-bun/test/errors.test.ts` “the errors this runtime makes have their code, data and text (#677)”，`bun/rutis-bun/test/bad-values.test.ts` “a method an object passed by reference lacks is no-method, saying what it has”
- Python：`python/rutis/tests/test_plain_texts.py` `test_no_method`，`python/rutis/tests/test_plain_texts.py` `test_too_many_arguments`，`python/rutis/tests/test_plain_texts.py` `test_a_missing_argument`
- Go：`crates/rutis-loader/tests/fault_texts.rs` `fault_messages_hold_none_of_the_platforms_words`，`go/rutis/internal/runtime/errors_text_internal_test.go` `TestARefusedCallNamesTheServiceMethodAndArgument`
- Rust（插件与嵌入）：`crates/rutis-loader/tests/fault_texts.rs` `fault_messages_hold_none_of_the_platforms_words`，`crates/rutis-bridge/src/marked.rs` `wrong_calls_name_the_service_method_and_argument`

### 4. `apply` 里出错

- TypeScript（Node）：`crates/rutis-host/tests/dev.rs` `dev_follows_a_node_project`，`crates/rutis-loader/tests/runtime_rows.rs` `a_failed_row_keeps_the_plugins_error`，`node/rutis-runtime/test/errors.test.mjs` “an apply failure crosses with the plugin's frames only (#677)”
- Bun：`crates/rutis-host/tests/dev.rs` `dev_follows_a_bun_project`，`bun/rutis-bun/test/runner.test.ts` “an apply failure crosses with the plugin's frames only (#677)”
- Python：`crates/rutis-host/tests/dev.rs` `dev_follows_a_python_project`
- Go：`crates/rutis-host/tests/dev.rs` `dev_rebuilds_a_go_project_only_for_its_sources`
- Rust（插件与嵌入）：`crates/rutis/tests/embedding_output.rs` `a_failed_start_reaches_the_error_sink_by_the_plugins_name`，`crates/rutis-loader/tests/waits.rs` `a_row_that_cannot_start_reaches_the_error_sink_by_its_name`

### 5. 语法 / 导入 / 编译错误

- TypeScript（Node）：`tests/e2e/tests/dev_loop.rs` `the_node_plugin_development_loop`，`crates/rutis-host/tests/check.rs` `check_fails_what_cannot_run`
- Bun：`tests/e2e/tests/dev_loop.rs` `the_bun_plugin_development_loop`，`bun/rutis-bun/test/plugin.test.ts` “a module that does not parse fails with its file and line”，`bun/rutis-bun/test/plugin.test.ts` “an import error names the submodule that threw, and a gone entry is said so (#677)”
- Python：`tests/e2e/tests/dev_loop.rs` `the_python_plugin_development_loop`，`python/rutis/tests/test_plain_texts.py` `test_a_syntax_error_says_its_file_and_line`，`python/rutis/tests/test_plain_texts.py` `test_the_submodule_that_raised_is_named`
- Go：`tests/e2e/tests/dev_loop.rs` `the_go_plugin_development_loop`
- Rust（插件与嵌入）：`crates/rutis/tests/compile_errors.rs` `author_mistakes_are_reported_in_the_authors_terms`

### 6. 跨语言传对面接不住的值

- TypeScript（Node）：`crates/rutis-loader/tests/bad_values.rs` `a_bad_value_fails_only_its_call_between_node_python_and_go`，`crates/rutis-bridge/tests/value_roundtrip.rs` `node_bad_values`，`crates/rutis-loader/tests/objects_cross.rs` `a_node_object_crosses_to_python`
- Bun：`crates/rutis-bridge/tests/value_roundtrip.rs` `bun_bad_values`，`crates/rutis-loader/tests/objects_cross.rs` `a_bun_object_crosses_to_node`，`bun/rutis-bun/test/bad-values.test.ts` “a value that cannot be sent carries code bad-value and its part (#677)”
- Python：`crates/rutis-loader/tests/bad_values.rs` `a_bad_value_fails_only_its_call_between_node_python_and_go`，`crates/rutis-bridge/tests/value_roundtrip.rs` `python_bad_values`，`crates/rutis-loader/tests/objects_cross.rs` `a_python_object_crosses_to_node`
- Go：`crates/rutis-loader/tests/bad_values.rs` `a_bad_value_fails_only_its_call_between_node_python_and_go`，`crates/rutis-loader/tests/objects_cross.rs` `a_go_parameter_that_takes_no_object_says_so`
- Rust（插件与嵌入）：`crates/rutis-bridge/src/session/rpc/tests/objects.rs` `a_rust_service_refuses_an_object_argument`，`crates/rutis-bridge/tests/value_roundtrip.rs` `rust_relays_a_thrown_value`

### 7. 事件监听器出错

- TypeScript（Node）：`crates/rutis-loader/tests/events_conformance.rs` `events_with_x_a_node_leaf_plugin`
- Bun：`crates/rutis-loader/tests/events_conformance.rs` `events_with_x_a_bun_plugin`
- Python：`crates/rutis-loader/tests/events_conformance.rs` `events_with_x_a_python_plugin`
- Go：`crates/rutis-loader/tests/events_conformance.rs` `events_with_x_a_go_plugin`
- Rust（插件与嵌入）：`crates/rutis-loader/tests/events_conformance.rs` `events_with_x_a_node_leaf_plugin`，`crates/rutis-bridge/src/events/errors.rs` `a_listener_error_records_its_row_and_event`

### 8. 调用一直不返回

- TypeScript（Node）：`crates/rutis-host/tests/status.rs` `status_says_what_a_running_host_does`，`node/rutis-runtime/test/output.test.mjs` “a method holding the runtime is said once; a short one is not”，`node/rutis-runtime/test/output.test.mjs` “a method awaiting for long does not hold the runtime, and is not said”，`node/rutis-runtime/test/output.test.mjs` “under a debugger nothing is said of a method holding the runtime”
- Bun：`bun/rutis-bun/test/output.test.ts` “a method holding the runtime is said once; a short one is not”，`bun/rutis-bun/test/output.test.ts` “a method awaiting for long does not hold the runtime, and is not said”，`bun/rutis-bun/test/output.test.ts` “under a debugger nothing is said of a method holding the runtime”
- Python：`crates/rutis-host/tests/status.rs` `status_says_what_a_running_host_does`，`crates/rutis-loader/tests/status.rs` `the_status_names_rows_services_runtimes_and_calls`，`python/rutis/tests/test_output.py` `test_a_method_holding_the_runtime_is_said_once`，`python/rutis/tests/test_output.py` `test_a_blocking_call_in_an_async_method_is_said`，`python/rutis/tests/test_output.py` `test_an_async_method_awaiting_long_is_not_said`，`python/rutis/tests/test_output.py` `test_under_a_debugger_nothing_is_said`
- Go：无测试
- Rust（插件与嵌入）：`crates/rutis/tests/embedding_output.rs` `an_apply_that_does_not_return_is_named`，`crates/rutis/tests/embedding_output.rs` `a_shutdown_that_runs_out_of_time_names_the_stuck_cleanup`

### 9. 互相调用、互相依赖

- TypeScript（Node）：`crates/rutis-host/tests/check.rs` `check_fails_on_cycles_and_clashes_and_suggests_misspelt_names`，`crates/rutis-loader/tests/sync_cycles.rs` `two_node_runtimes_calling_each_other`
- Bun：`crates/rutis-loader/tests/sync_cycles.rs` `an_emit_listener_run_while_a_call_waits`
- Python：`crates/rutis-loader/tests/sync_cycles.rs` `one_node_and_two_python_runtimes`，`python/rutis/tests/test_plain_texts.py` `test_a_sync_wait_cycle_prints_its_message_alone`
- Go：`crates/rutis-loader/tests/sync_cycles.rs` `two_node_runtimes_waiting_on_each_other_through_go`
- Rust（插件与嵌入）：`crates/rutis-loader/tests/waits.rs` `rows_that_inject_each_other_say_so`，`crates/rutis-loader/tests/check.rs` `rows_that_inject_each_other_are_a_cycle`

### 10. 运行时进程中途退出

- TypeScript（Node）：`crates/rutis-loader/tests/fault_containment.rs` `node_a_runtime_that_exits_blames_no_other_row`，`crates/rutis-host/tests/unhandled.rs` `a_plugin_ending_its_process_is_named`
- Bun：`crates/rutis-loader/tests/fault_containment.rs` `bun_a_runtime_that_exits_blames_no_other_row`，`crates/rutis-host/tests/unhandled.rs` `a_plugin_ending_its_process_is_named`
- Python：`crates/rutis-loader/tests/fault_containment.rs` `python_a_runtime_that_exits_blames_no_other_row`，`crates/rutis-host/tests/runtimes.rs` `a_killed_runtime_is_reported_with_how_it_ended`，`tests/e2e/tests/runtime_crash.rs` `a_killed_runtime_fails_its_calls_and_a_restart_recovers`，`crates/rutis-host/tests/unhandled.rs` `a_plugin_ending_its_process_is_named`
- Go：`crates/rutis-loader/tests/go_rows.rs` `a_crash_stops_only_its_runtimes_rows_and_is_not_restarted`，`crates/rutis-host/tests/runtimes.rs` `a_go_runtime_restarted_on_failure_brings_its_rows_back`
- Rust（插件与嵌入）：不适用

### 11. 下断点、看日志

- TypeScript（Node）：`crates/rutis-host/tests/debug.rs` `dev_debug_opens_the_node_inspector`，`crates/rutis-host/tests/output.rs` `plugin_lines_written_in_pieces_come_out_whole`，`node/rutis-runtime/test/output.test.mjs` “each line a row's code writes starts with the row's name”，`crates/rutis-host/tests/output.rs` `prefix_output_starts_each_plugin_line_with_its_row`
- Bun：`crates/rutis-host/tests/debug.rs` `dev_debug_opens_the_bun_inspector`，`bun/rutis-bun/test/output.test.ts` “each line a row's code writes starts with the row's name”
- Python：`crates/rutis-host/tests/debug.rs` `dev_debug_stops_python_at_breakpoint`，`python/rutis/tests/test_output.py` `test_each_line_a_row_writes_starts_with_its_name`，`crates/rutis-host/tests/output.rs` `prefix_output_starts_each_plugin_line_with_its_row`
- Go：`crates/rutis-host/tests/debug.rs` `dev_debug_runs_the_binary_under_delve`
- Rust（插件与嵌入）：`crates/rutis/src/error.rs` `default_sink_lines_start_with_the_name_they_are_about`

### 12. 配置写错

- TypeScript（Node）：`crates/rutis-host/tests/check.rs` `check_fails_a_config_that_does_not_match_the_schema`，`crates/rutis-host/tests/check.rs` `check_warns_of_extra_config_keys`，`node/rutis/test/plugin.test.mjs` “the host fills the config defaults and refuses a config the schema refuses”
- Bun：`bun/rutis-bun/test/sdk.test.ts` “the host fills the config defaults and refuses a wrong config”
- Python：`python/rutis/tests/test_config.py` `test_a_bad_config_crosses_with_its_code_and_issues`，`python/rutis/tests/test_config.py` `test_an_extra_key_is_warned_about_once`
- Go：`go/rutis/internal/runtime/config_internal_test.go` `TestATypedConfigIsDecoded`，`go/rutis/internal/runtime/config_internal_test.go` `TestAConfigTheTypeDoesNotTakeIsSaidPlainly`，`go/rutis/rutistest/config_test.go` `TestATypedConfigInTheTestTool`
- Rust（插件与嵌入）：`crates/rutis-loader/tests/config_rows.rs` `a_rust_row_is_filled_and_validated`，`crates/rutis-loader/tests/config_rows.rs` `extra_keys_are_warned_about_once`，`crates/rutis/tests/config_cases.rs` `every_case_of_the_shared_list_passes`

### 13. 两个插件提供同名服务

- TypeScript（Node）：`crates/rutis-host/tests/clash.rs` `a_service_provided_after_loading_twice_fails_the_later_row`，`crates/rutis-loader/tests/fault_containment.rs` `node_a_clashing_provide_fails_only_its_row`，`crates/rutis-host/tests/check.rs` `check_fails_on_cycles_and_clashes_and_suggests_misspelt_names`
- Bun：`crates/rutis-loader/tests/fault_containment.rs` `bun_a_clashing_provide_fails_only_its_row`，`bun/rutis-bun/test/runner.test.ts` “a row that cannot load or provides a taken service is told so plainly (#677)”
- Python：`crates/rutis-loader/tests/fault_containment.rs` `python_a_clashing_provide_fails_only_its_row`，`python/rutis/tests/test_plain_texts.py` `test_a_service_provided_twice_names_the_row`
- Go：`go/rutis/internal/runtime/errors_text_internal_test.go` `TestADuplicateProviderIsNamedByItsRow`
- Rust（插件与嵌入）：`crates/rutis-loader/tests/check.rs` `rows_declaring_one_service_in_one_scope_clash`，`crates/rutis-bridge/tests/marked_services.rs` `a_second_service_of_the_same_name_is_refused`

### 14. 查看现在的运行状态

- TypeScript（Node）：`crates/rutis-host/tests/status.rs` `status_says_what_a_running_host_does`
- Bun：无测试
- Python：`crates/rutis-loader/tests/status.rs` `the_status_names_rows_services_runtimes_and_calls`
- Go：无测试
- Rust（插件与嵌入）：`crates/rutis/tests/embedding_output.rs` `diagnostics_print_by_name_without_rutis_own_plugins`，`crates/rutis-loader/tests/status.rs` `the_status_names_rows_services_runtimes_and_calls`

<!-- faults:end -->
