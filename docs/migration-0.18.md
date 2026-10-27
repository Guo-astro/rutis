# 迁移到 0.18

[English](migration-0.18.en.md)

0.18 让出错的时候看得懂（[诊断设计](design-diagnostics-2026-10-26.md)）。本页随 0.18 的改动逐项补齐；下面是已经完成的部分。

## `rutis-host check`：依赖环与同名提供改为报错（#676）

0.17.2 里这两种情况只写告警，退出码不变；0.18 起写 `rutis-host: error: …`，以 1 退出（`--strict` 与否都一样）。在 CI 里跑 `check` 的配置，以前通过的可能会失败：

- **几行的 `inject` 互相等对方提供的服务（一个环）**：这几行谁也起不来。

  ```
  rutis-host: error: rows a, b wait for each other (a waits for bsvc from b, b waits for asvc from a): none of them starts; break the cycle in their inject
  ```

  改法：从其中一行的 `inject` 里去掉它其实不需要的服务；两边都需要时，把共同的部分拆成第三行，让它们都等这一行。

- **几行在同一个作用域里声明提供同一个服务**：只有先起来的那行提供，其余的失败。

  ```
  rutis-host: error: rows one, two all provide llm: only the first to start provides it, the others fail; keep one, or isolate the name in each row
  ```

  改法：只留一行；两份都要时，用 `isolate` 把这个名字隔离开：`{ "llm": "标签" }` 让提供它的行和用它的行带同一个标签，各用各的一份（`{ "llm": true }` 是这一行自己用的一份，别的行用不到）。

升级前用 0.17.2 跑一遍 `rutis-host check`：0.17.2 对这两种情况写的告警，就是 0.18 会报的错。

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
- `str(SyncWaitCycle)` 只是消息，前面不再有 `SyncWaitCycle: `（与其他错误相同）。
- `raise … from cause` 的 `cause` 作为错误图里的 `cause` 一起发出（与 JavaScript 错误的 `cause` 相同）；加载中没处理的错误（`UnhandledError`）以原错误为 `cause`，原错误自己的 `cause` 和 `AggregateError` 的 `errors` 也一起发出。
- 导入插件模块（或它导入的模块）时出错，抛出 `LoadError`（`from rutis import LoadError`，是 `Exception` 的子类），写明出错的文件和行号；入口模块本身不存在时仍是原来的 `ModuleNotFoundError`。

### Go

- `Parallel` 在真实宿主里返回 `*rutis.AggregateError`（以前只在测试工具里如此），`errors.As` 成立，`Data` 不为空。
- 开发者见到的错误类型打印为 `*rutis.RemoteError`、`*rutis.AggregateError` 等，不再是 `*peer.RemoteError`。
- 多余的参数报 `bad-argument`；少给的参数照旧以零值补上。

### Kotlin

- 没写进 `inject` 与服务不在时，抛出新的 `NotDeclaredException`（是 `IllegalArgumentException` 的子类）与 `UnavailableException`（是 `IllegalStateException` 的子类）；宿主断开时抛出 `HostGoneException`（是 `IOException` 的子类）。它们带码与 `data`（`code`、`data` 属性），接住原来那几个类型的代码照常工作。

### Rust 插件

- 写错时的编译错误改为说人话：服务方法的参数不能跨语言、错误类型没有 `Display`、服务类型不是 `Send + Sync`、事件类型没实现 `rutis::Event`、`type Deps` 里不是 `Arc<T>` 等时，第一行写明哪里不对、怎么改，不再出现宏和 trait 的内部名字。没有代码要改。
- `ctx.on(async |e, _| …)` 仍报 E0282（推不出参数类型）；写成 `async |event: Arc<Ping>, _: Call<Ping>| …` 或 `|event, _| async move { … }`，见 `Listener` 与 `Ctx::on` 的文档。

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

## Node、Bun、Python 运行时：没处理的错误的处理方式改变（#688）

0.17 里 Node、Bun 插件没处理的 promise 拒绝（或定时器里抛出的异常）让它所在的运行时进程退出，同一运行时里的行一起停下等待；Python 任务里没人取的异常只被 asyncio 记一行日志，这一行照常运行。0.18 起：

- 进程不退出。追得到产生它的那一行时（`rutis-host dev` 下的 Node；Python 里在某一行里运行的任务和回调），这一行失败，写明原因和位置；同一运行时里的其他行继续。

  ```
  weather: failed: unhandled rejection: Error: no region
      at Timeout._onTimeout (file:///…/weather.mjs:12:9)
  ```

- 追不到时（`rutis-host run` 下的 Node，Bun，Python 里不在任何一行里运行的代码）写一行来源不明，哪一行都不失败：

  ```
  runtime node: an unhandled error from an unknown row: Error: no region
      at …
  ```

  下面只列插件自己的帧；一帧都不是插件的时候列出完整堆栈，那是找来源的唯一线索。

- 插件调用 `process.exit` / `os._exit` 时进程照常结束，运行时先写一行 `runtime node: row weather called process.exit(1)`。

要改的：

- **Python**：以前一个任务出错后这一行照常运行，现在这一行失败（宿主重启它，重启也失败）。自己启动的任务，处理或取走它的异常（`await` 它，或 `add_done_callback` 里读 `exception()`）。
- **Node、Bun**：以前靠"出错就让运行时退出、宿主按 `restart` 重启运行时"恢复的项目，现在不会有运行时退出：`restart: "on-failure"` 不再因此重启运行时，`restart: "exit"` 的宿主不再因此以 3 退出。要在出错后恢复，在插件里处理错误；确实要结束进程的，显式调用 `process.exit`。

详见插件 API 指南的[同一运行时里的插件共用一个进程](guide/plugin-api.md#同一运行时里的插件共用一个进程)。

## 同一监听器上两个条目接受同一个 token：启动即报错（#702）

同一个监听器上，监听的节点行和连进来的运行时（`runtimes.remote` 的 `listen` + `instances`）会接受同一个 token 时（例如都没有单独的 token、都退回 `RUTIS_TOKEN`），0.17 照常启动，但连接到达时以 403 拒绝（分不出是给谁的），运行时每 30 秒重试。0.18 起 `run`、`check`、重新读配置在启动前报错。同一个监听器上任意两个条目都一样：两个连进来的运行时（0.17 同样以 403 拒绝）、两个监听的节点行（0.17 里连接都到了后一行，前一行永远连不上）。

```
rutis-host: listener public: link row main and remote runtime phone both accept RUTIS_TOKEN, so a connection presenting it cannot be told apart: set RUTIS_TOKEN_MAIN or RUTIS_TOKEN_PHONE to a token of its own
```

改法：按提示给其中一个设单独的 `RUTIS_TOKEN_<名字>`（连进来的运行时那一边也出示这个新 token）。这样的配置在 0.17 里本来就连不上，改好之后才能用。

## 带方法的对象传给任何语言（#687）

0.17 里带方法的对象（类的实例等）只能传给 Rust；传给 Node、Bun、Python、Go、Swift、Kotlin 时这次调用失败（`argument 1 of warmer.warm: an object with methods cannot cross to py; pass plain data or a function instead`）。0.18 起它们按引用传给任何语言：对方拿到代理，方法调用的是原对象（[设计](design-object-refs-2026-10-27.md)，规则见插件 API 的[带方法的对象](guide/plugin-api.md#带方法的对象)）。什么按引用、什么复制没有变：JS 的类实例和带函数值属性的普通对象，Python 里 dataclass 以外的实例，Go 的 `rutis.Ref(v)`，Swift、Kotlin 的 `@RutisService` 实例。Rust 宿主和 Cordis 绑定收到的值不变。

要改的：

- **靠这个失败的代码**：以前先传对象、失败后改传数据（`catch` 里转成 dict 或普通对象再调一次）的代码，现在不会走到 `catch`：对方收到的是代理，方法都是异步调用，没有数据字段。对方要的是数据时，直接传数据。
- **收的一方的参数类型**：Go、Swift、Kotlin 里参数是数据类型时，收到对象的调用以 `bad-argument` 失败（以前是发送方的 `bad-value`），错误写明对象要怎样收：Go 是函数字段的结构体或 `any`，Swift 是 `@RutisClient` 协议或 `Any`，Kotlin 是 `@RutisClient` 接口或 `Any?`。Rust 的 `#[rutis::service]` 参数不收对象，以 `bad-argument` 失败，改传函数。
- **两端都要升级**：0.18 之前的运行时不收对象。0.18 的宿主要把对象转给这样的运行时时，这次调用以 `bad-value` 失败，会话继续：

  ```
  chat: warmer.warm: argument 1 cannot be sent: an object from node cannot cross to py: py does not take objects (older than rutis 0.18); upgrade it
  ```

  直接连着旧的一端时，SDK 自己拒绝：`… cannot cross to main; pass plain data or a function instead, or upgrade main to rutis 0.18`。0.17.2 的宿主不认得 0.18 运行时收对象，它下面的运行时之间传对象照旧以 0.17.2 的文字失败；升级宿主。

### `undefined`

`undefined` 只在作为整个参数或整个返回值时保留；在数组、对象、Map、Set 里任何一层都是 `null`（Python 收到 `None`），各语言一样。以前作为参数的数组里的 `undefined` 保留成特殊值（Python 收到一个占位值，JS 收到 `undefined`），与文档不符。比较数组元素是不是 `undefined` 的 JS 代码改为同时看 `null`（`x == null`）。

### Swift

- 参数或 `@RutisClient` 方法的返回值类型写 `@RutisClient` 协议（`any Cache`）时，收到对象拿到这个协议的代理；类型写 `Any` 时拿到 `Rutis.Client`（`call(_:_:as:)`、`call(_:values:as:)`）。数组、字典、事件参数里的对象经 `Any` 拿到 `Rutis.Client`；`[any Cache]` 不支持。
- 只有写在文件顶层的 `@RutisClient` 协议能收对象；嵌在别的类型里的协议照常可以 `ctx.use`，但用作参数类型时收不了对象（`bad-argument`）。要收对象，把协议移到文件顶层。
- `RutisProxy` 多了 `rutisClient`：代理再交出去时发出原来那个对象。自己实现 `RutisProxy` 的代码（不经 `@RutisClient`）要加上它。

### Kotlin

- 新类型 `arcships.rutis.RemoteObject`：类型为 `Any?` 的参数收到对象时拿到它，`call(method, vararg args)` 挂起，`callBlocking(method, vararg args)` 阻塞，`release()` 立即交还引用。这是 0.18 里插件代码为对象要用的唯一新类型（`arcships.rutis.ObjectProxy` 也是公开的，但只由 KSP 生成的代理实现，插件代码不用它，和 `ServiceHandle` 一样）。
- 参数或 `@RutisClient` 方法的返回值类型写 `@RutisClient` 接口时，收到对象拿到 KSP 生成的代理。
- 以前把 `Any?` 参数当作只会收到数据（`Map`、`List`、基本类型）的代码，现在还可能收到 `RemoteObject`。

## Rust 嵌入（行为变化）（#678）

0.17 里嵌入 rutis 的 Rust 程序默认几乎什么都不打印：没人等的插件启动失败、loader 的行加载失败、语言运行时的进程结束，都只在没人读的返回值里。0.18 起默认的错误汇集（`Ctx::root()`、`Ctx::root_with(handle)` 装的 `default_sink()`）把它们写到标准错误，每行以插件、行或运行时的名字开头（[默认输出](guide/rust-host.md#默认输出)）：

```
weather: failed: no API key
runtime py: down (runtime process exited with exit status: 1)
```

- **默认输出不再以 `[rutis]` 开头。** 等待的提示改为以插件名开头：`printer: still waiting after 10s for app::Greeting: it starts once that is provided`（0.17.2 是 `[rutis] plugin printer is still waiting …`）。rutis 自己的插件（链路、运行时等）不再报等待，提示里也不出现它们的内部名字。按这些文字匹配标准错误的脚本要改。
- **新的错误汇集变体**：`ErrorKind::StartFailed { plugin, error }`、`ErrorKind::RuntimeDown { runtime, reason }`、`ErrorKind::NotServedByName { service }`（标了 `#[rutis::service]` 的类型被提供、而程序没链接 rutis-bridge 时，每个服务报一次）。自己装了汇集（`Ctx::root_with_sink`）的程序也会收到它们；不想要的按变体过滤掉。等插件的 `view.await` 照样收到错误，所以既等结果又用默认汇集的程序会看到同一个失败两次，要么不打印 `view.await` 的错误，要么在汇集里滤掉 `StartFailed`。
- **要关掉**：换掉汇集，见[默认输出](guide/rust-host.md#默认输出)里的例子。
- **rutis-host**：状态行不变，它不打印 `StartFailed`、`RuntimeDown`（状态行已经写了）。变的是：它交给默认汇集的其余错误（例如清理出错）不再以 `[rutis]` 开头；Go 运行时被强制结束后那一句 `runtime <名字>: stopping after killing it: …` 里的超时文字改为写明卡住的插件。
- **同一件事可能打印几行**：插件启动失败时先报回滚中清理的错误，再报 `StartFailed`（它的错误里也有那些清理错误）；父插件因子插件失败而失败时两者各报一次；几个错误聚在一起时分几行打印。
- **`shutdown()` 不变**，仍然不限时，等多久由宿主决定。`shutdown_with_timeout` / `dispose_with_timeout` 到时的错误改为写明还没结束的插件和清理（`not finished after 3s: weather: still running the cleanup registered at src/weather.rs:12:9`），不再是 `fiber PluginId(1) generation 0 still Unloading after …`；`DisposeWaitError::TimedOut` 多了字段 `stuck: Vec<Stuck>`，列出全部字段解构它的代码加上 `..`。
- `root.diagnostics()` 实现了 `Display`（一项一行，用名字，默认不列 rutis 自己的插件）；`PluginDiagnostics` 多了 `row`、`platform` 两个字段。

## Rust 嵌入：`reconcile` 不等还在启动的运行时（#678）

0.17 里，运行时还在启动时，loader 解析它的行要等它起来，`reconcile` 也就跟着等；一个运行时卡住（比如在等调试器），所有行都不加载。0.18 起不等：它的行先在等这个运行时，等它起来、重新解析之后再启动；其他行照常启动，`reconcile` 照常返回。

看得到的不同：在运行时起来之前调用 `reconcile`，报告里没有这些行装载的结果（`failures` 里也不会有它们的错误），这些行先停在等运行时。`Loader::check`、`Loader::schema_of` 这时也拿不到它们的 Schema 和声明。要和以前一样拿到完整的报告，先等运行时：

```rust
let runtimes = [node.handle(), python.handle()];
root.plugin(node);
root.plugin(python);
for runtime in &runtimes {
    runtime.ready().await;
}
let report = handle.reconcile(layers, None).await?;
```

`rutis-host` 不受影响：它仍然在所有运行时起来、每一行都装载过之后才算启动。
