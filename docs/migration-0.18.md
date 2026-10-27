# 迁移到 0.18

[English](migration-0.18.en.md)

0.18 让出错的时候看得懂（[诊断设计](design-diagnostics-2026-10-26.md)）。本页按改动分节；各节随 0.18 的 issue 补齐。

## 错误（#677）

### 所有语言

- **错误类型名与字段不变。** `BindingError`、`AggregateError`、`SyncWaitCycle` 等名字照旧，交给代码的错误对象没有新字段。
- **出错文字会变。** 按文字匹配错误的代码会失效，请改为按错误码判断（见下一条）。文字只用开发者写过的东西：行名、服务名、方法名、第几个参数、代码里的文件和行号，以及怎么改；不再出现 rutis、Cordis、binding、session、内部 id（`row:23.2`）和 SDK 的内部类型。主要的变化：

  | 0.17 | 0.18 |
  | --- | --- |
  | `plugin failed: KeyError: 'region'` | `KeyError: 'region'` |
  | `weather row:23.2.tomorrow is not provided by the rutis host`、`unknown service method …` | `weather has no method tomorow (methods: today, tomorrow)` |
  | `2 arguments given, 1 expected`、`expected at most 1 arguments, received 2` | `weather.tomorrow takes 1 argument, got 2` |
  | `invalid type: string "x", expected u32` | `weather.tomorrow: argument 1 should be an integer, got "x"` |
  | `invalid type: null`（缺参数） | `weather.tomorrow: argument 2 is missing` |
  | `argument 2 of weather.store: …`、`the result of weather.store: …` | `weather.store: argument 2 cannot be sent: …`、`weather.store: the result cannot be sent: …` |
  | `Out of range float values are not JSON compliant`（Python 发出 NaN） | `weather.store: argument 1 cannot be sent: NaN cannot cross processes` |
  | `Rust process disconnected`、`the rutis host disconnected` | `the host disconnected` |
  | `the session ended` | `the connection ended`（`rutis-host` 的 `link down (the connection ended)` 同样） |
  | `2 listeners of weather/asked failed`；一个时 `1 listeners …` | 一个时 `1 listener of weather/asked failed` |
  | `service "forecast" has been registered at <root>`、`service forecast is already provided by row a: row b cannot provide it too` | `forecast is already provided by a`（宿主提供的写 `by the host`） |
  | TypeScript 抛出字符串：`ThrownValue: need 5 words` | `"need 5 words"` |
  | 默认导出不是 `definePlugin` 的结果：`cannot get property "use" without inject` | `the default export of src/index.ts is not a plugin made with definePlugin` |
  | 子模块导入时出错，报在入口文件 | `cannot load src/util.ts: SyntaxError: … (src/util.ts:3:7)`（错误名 `LoadError`；Python 同样） |
  | `entry file is gone` | `the entry file src/index.ts is gone` |
  | `apply failed (…) and rollback failed (…)` | `the change failed (…), and the rows as they were did not start again either (…)` |
  | `and 17 plugin(s) waiting for them` | `and 5 rows waiting for them`（数的是行） |

- **rutis 自己的 `BindingError` 带错误码。** 码给程序判断，不出现在文字里；细节在 `data`。各语言读法与插件抛出的错误相同：JavaScript `error.code` / `error.data`，Python `error.code` / `error.data`，Go `*rutis.RemoteError` 的 `Code` / `Data`，Swift、Kotlin 错误的 `code` / `data`，Rust `session::Error::code()` / `data()`：

  | code | 什么时候 | `data` |
  | --- | --- | --- |
  | `not-declared` | 用了没写进 `inject` 的服务 | `{service}` |
  | `unavailable` | 服务现在没有提供者 | `{service, reason?}` |
  | `no-method` | 方法不存在 | `{service, method, methods}`；对象上的方法是 `{object, method, methods}` |
  | `bad-argument` | 参数个数或类型不对 | 个数：`{service, method, expected, received}`；类型或缺少：`{service, method, argument, expected}`（`argument` 从 1 数起） |
  | `bad-value` | 值传不过去，或消息超过上限 | `{part, argument?, limit?}`（`part` 是 `argument`、`result` 或 `error`） |
  | `peer-gone` | 调用进行中对面的进程退出或断开 | 宿主一侧 `{runtime, reason}`；插件一侧 `{reason}` |

  协议层面的拒绝（一端没按约定办事）没有码。

  ```ts
  // 0.17
  if (/is not available/.test(error.message)) { … }
  // 0.18
  if (error.code === 'unavailable' || error.code === 'not-declared') { … }
  ```

  ```python
  # 0.18
  except BindingError as error:
      if error.code == "no-method": ...
  ```

  ```go
  // 0.18
  var remote *rutis.RemoteError
  if errors.As(err, &remote) && remote.Code == "peer-gone" { … }
  ```

- **打印的错误写出是谁、在做什么、在哪里。** `rutis-host run` / `dev` 打印的行失败先说失败在哪一步，下面缩进写出错的行和插件语言给出的位置，跨语言时再写调用方的位置：

  ```text
  weather: failed in apply: KeyError: 'region'
      in weather:
      File "weather.py", line 12, in apply
  chat: call to forecast.tomorrow failed: KeyError: 'region'
      in weather:
      File "weather.py", line 31, in tomorrow
      called from:
      at ask (src/chat.ts:14:36)
  ```

  解析这些输出的脚本要按新格式改。跨进程发出的堆栈只有插件自己的帧（不再带 SDK、Cordis 的帧）。

### Node、Bun

- **收到的跨语言错误，`error.stack` 是调用方自己的堆栈**（以前被换成对面的）。对面的堆栈由 rutis 打印；把这个错误再抛出时，SDK 原样带上对面的错误。依赖 `error.stack` 里有对面帧的代码要改为看 rutis 的输出。
- **测试工具（`@arcships/rutis/testing`）给出与真实运行时相同的文字、码与 `data`**：没写进 `inject`、服务不在、方法不存在、值传不过去（参数从 1 数起，以前是 `svc.m argument 0: …`）。

### Python

- 宿主服务的方法在堆栈里显示为 `weather.today`，不再是 `HostProxy.__getattr__.<locals>.call`；返回"没有值"的调用得到 `None`，不会再见到 `UNDEFINED`。
- 宿主断开时调用抛出 `ConnectionError`，带码 `peer-gone`。
- 导入插件模块（或它导入的模块）时出错，抛出 `LoadError`，写明出错的文件和行号；入口模块本身不存在时仍是原来的 `ModuleNotFoundError`。

### Go

- `Parallel` 在真实宿主里返回 `*rutis.AggregateError`（以前只在测试工具里如此），`errors.As` 成立，`Data` 不为空。
- 开发者见到的错误类型打印为 `*rutis.RemoteError`、`*rutis.AggregateError` 等，不再是 `*peer.RemoteError`。
- 多余的参数报 `bad-argument`；少给的参数照旧以零值补上。

### Kotlin

- 没写进 `inject` 与服务不在时，仍分别抛出 `NotDeclaredException`（`IllegalArgumentException`）与 `UnavailableException`（`IllegalStateException`），现在带码与 `data`。宿主断开时是 `HostGoneException`（仍是 `IOException`）。

### Rust 插件

- 写错时的编译错误改为说人话：服务方法的参数不能跨语言、错误类型没有 `Display`、服务类型不是 `Send + Sync`、事件类型没实现 `rutis::Event`、`type Deps` 里不是 `Arc<T>` 等时，第一行写明哪里不对、怎么改，不再出现宏和 trait 的内部名字。没有代码要改。
- `ctx.on(async |e, _| …)` 仍报 E0282（推不出参数类型）；写成 `async |event: Arc<Ping>, _: Call<Ping>| …` 或 `|event, _| async move { … }`，见 `Ctx::on` 的文档。

### Rust 宿主

- **`ErrorKind::PluginFailed` 显示时没有 `plugin failed: ` 前缀。**
- **插件的错误不再是字符串。** 行失败、调用插件的服务失败时，用 `rutis_bridge::thrown(&error)` 读出插件抛出的错误（`name`、`message`、`code()`、`data()`、`stack()`、`site()`：是哪一行、在做什么）。打印用 `rutis_bridge::report::report(&error)`，与 `rutis-host` 的写法相同。loader 的 `Failure`、`ReconcileReport` 不变；要结构的从行的状态（`EntryInfo` 的快照错误）读。
- **`rutis_bridge::session::Error` 新增分支 `Binding { code, message, data }`。** rutis 拒绝一次调用时用它：方法不存在、参数不对、服务不在（带码），以及协议层面的拒绝（`code` 为 `None`）。`Error::Value` 只表示值传不过去。对 `session::Error` 逐个分支 `match` 的代码要加上这一支；判断是哪一类时用 `Error::code()`（`Value` 为 `bad-value`，`Transport` 为 `peer-gone`）：

  | 0.17 | 0.18 |
  | --- | --- |
  | `Err(Error::Value(m)) if m.starts_with("unknown service method")` | `Err(e) if e.code() == Some("no-method")` |
  | `Err(Error::Value(m)) if m.contains("is not available")` | `Err(e) if e.code() == Some("unavailable")` |
  | `Err(Error::Value(m))`（值传不过去） | 不变；或 `e.code() == Some("bad-value")` |

- `SyncWaitCycle` 显示时没有 `synchronous wait cycle: ` 前缀，写成 `weather.today would wait on itself: chat → weather → chat (synchronous calls)`。
- 一行因为服务已有提供者而失败时，它的错误是名为 `ProvideError` 的插件错误（以前是字符串）。`BindingError`、`LoadError`、`ProvideError` 和抛出的字符串、对象，显示时只写消息，前面没有 `<名字>: `。
- `build::Exports` 生成的类型声明文件头改为 `// Generated from the service declarations; do not edit.`；比对生成文件的检查要重新生成一次。

### 协议

- 错误节点多两个可选字段：`site`（出错的行、在做什么）与 `rethrown`（再抛出它的调用方的帧）。旧的一端忽略它们；不升协议版本，与 0.17 照常相遇。
