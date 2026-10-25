# 迁移到 0.17

[English](migration-0.17.en.md)

0.17 只改 Rust 的公开 API：插件只有一种写法，宿主用的东西放进 `rutis::host`，事件方法搬到 `Ctx` 上，错误只有一种类型 `rutis::Error`。没有新能力，也没有行为变化：迁移后能编译的代码，运行起来和 0.16 一样。

受影响的只有写 Rust 插件和用 Rust 嵌入 rutis 的项目。TypeScript、Python、Go、Swift、Kotlin 的 SDK、运行时、协议和 `rutis.json` 都不变。dylib 插件照常用新的 SDK 重新构建。

每个旧名字的去向见设计的[附录 A](design-rust-api-convergence-2026-10-24.md#附录-a名字去向)；新的写法见 [写一个 Rust 插件](guide/rust-plugin.md) 和 [在 Rust 应用里嵌入](guide/rust-host.md)。

## 第一步：运行迁移工具

```bash
cargo install rutis-migrate
rutis-migrate 0.17 path/to/crate            # 只显示差异
rutis-migrate 0.17 path/to/crate --write    # 写回文件，再运行 cargo check
```

在 rutis 仓库里也可以 `cargo run -p rutis-migrate -- 0.17 path/to/crate --write`。

- 它改 `PATH` 下用到 rutis 的 `.rs` 文件（跳过 `target`、隐藏目录和 `node_modules`）。语法树只用来定位，替换在原文上做，注释和格式都保留。
- 不带 `--write` 时只打印差异，什么都不改。带 `--write` 时写回文件，然后在 `PATH` 下运行 `cargo check --all-targets`，列出剩下的错误（`--no-check` 不运行）。
- 改不了的地方插入一行 `// TODO(rutis 0.17): …`，说明要怎么改；剩下的编译错误就在这些注释附近。`grep -rn "TODO(rutis 0.17)"` 列出全部。
- 已经是 0.17 写法的文件不再改（只把 rutis-loader 根上移走的名字改成模块路径），所以可以重复运行。

它自动改的：

| 0.16 | 0.17 |
| --- | --- |
| 类型位置的 `CordisError` | `rutis::Error`；`Arc<CordisError>` 和变体路径（`CordisError::Closed`）是 `host::ErrorKind` |
| `.map_err(\|e\| CordisError::PluginFailed(Box::new(e)))?` | `?`；`CordisError::PluginFailed(e.into())` 写成 `Error::from(ErrorKind::PluginFailed(e.into()))`（`e` 可能是字符串，`?` 接不住） |
| `impl Plugin` | `impl host::DynPlugin`（就是原来的 trait 改了名字），`injects` 返回 `Vec<TypeKey>` |
| `Box<dyn Plugin>`、`Arc<dyn Plugin>`、`P: Plugin`、参数位置的 `impl Plugin` | `DynPlugin` |
| `impl TypedPlugin` | `impl Plugin`，`apply` 写成 `async fn`，末尾的 `Ok(Effect::Done)` 写成 `Ok(())`；`Typed::new(p)` 写成 `p` |
| `ctx.effect(\|\| Effect::AsyncDisposer(…))`、`Effect::Disposer(…)` | `ctx.effect(\|\| async move { … })` |
| `ctx.events().on(&ctx, &key, \|c, e, call\| Box::pin(async move { … }))`，以及 `on_sync`、`on_opt`、`on_pattern`、`once` | `ctx.on::<E>(\|e, call\| async move { … })`、`ctx.on_sync`、`ctx.listen::<E>()…`；块里的 `c` 写成 `call.ctx()` |
| 手写 `impl Listener<E>` 的结构体 | 结构体的 `async fn on_event(…)` 方法，登记处改成调用它的闭包 |
| `match err { CordisError::X => … }`、`matches!(err, …)` | `match &*err { ErrorKind::X => … }`：`Error` 解引用成宿主看到的 `ErrorKind` |
| `events.emit(&ctx, &key, Arc::new(v))`，以及 `parallel`、`serial`；`bail_sync`；`waterfall(&ctx, &key, &e, t)` | `ctx.emit(v)` 等；`ctx.bail(v)`；`ctx.waterfall(e).terminal(t)` |
| `provide_as(TypeKey::of::<T>(), arc)`、`get_as(TypeKey::of::<T>())` | `provide_as::<T>(arc)`、`get::<T>()`；带名字的键写成 `provide_at`、`get_at`、`require_at` |
| `ServiceKey`、`DepKey` | `TypeKey`、`host::Key` |
| 从 `rutis` 根上移走的名字 | `rutis::host::…`、`rutis::host::diagnostics::…`、`rutis::events::…`；用到宿主方法的文件加 `use rutis::host::CtxHost;` |
| 从 `rutis_loader` 根上移走的名字 | 模块路径（`rutis_loader::report::…` 等，见下面的 [rutis-loader](#rutis-loader)） |
| `builtins.register_fn(name, \|c\| Ok(Box::new(P::new(c))))` | `builtins.register(name, \|c\| Ok(P::new(c)))` |

它留下 TODO 的：返回 `Effect::Many` 的清理、`apply` 返回的清理、推不出事件类型的键和前缀、它改不了的监听器结构体、`Gate` / `Keyed` / `KeyedGate` / `Typed::with_keys`、按每个实例构造工厂的 `register_with`（见 [rutis-loader](#rutis-loader)）、以运行时才定的名字发出的事件（见[事件](#事件)）、`provide_at` 收到的未带类型的 `TypeKey`。把 `rutis::Error` 放进 `Arc<ErrorKind>` 的地方没有 TODO，编译错误会指出来，按下面的[错误](#错误)改。

## 之后手工改的

### 插件

工具跑完以后，旧插件以 `host::DynPlugin` 的身份照常运行。要换成新的 `Plugin`：`injects()` 里列的类型移进 `type Deps`，`apply` 里对应的 `require` 改成参数，`apply` 返回的 `Effect` 改成在 `apply` 里调用 `ctx.effect`：

```rust
// 0.16
impl Plugin for Weather {
    fn name(&self) -> &str { "weather" }
    fn injects(&self) -> &[TypeKey] { &self.injects } // [TypeKey::of::<dyn Llm>()]
    fn apply<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, Result<Effect, CordisError>> {
        Box::pin(async move {
            let llm = ctx.require_as::<dyn Llm>(TypeKey::of::<dyn Llm>())?;
            let file = std::fs::read_to_string(&self.path)
                .map_err(|e| CordisError::PluginFailed(e.into()))?;
            ctx.provide(Forecast::new(llm, file))?;
            Ok(Effect::Done)
        })
    }
}

// 0.17
impl Plugin for Weather {
    type Deps = Arc<dyn Llm>;

    async fn apply(&self, ctx: &Ctx, llm: Arc<dyn Llm>) -> Result {
        let file = std::fs::read_to_string(&self.path)?;
        ctx.provide(Forecast::new(llm, file))?;
        Ok(())
    }
}
```

- `name` 可以不写，默认是类型名的最后一段。
- 按名字依赖服务的插件（`Keyed`、`Typed::with_keys`）保持 `DynPlugin`：`injects()` 里列出 `Key::<T>::named("x").erase()`，`apply` 里用 `ctx.require_at(&Key::named("x"))` 读（[按名字依赖](guide/rust-host.md#按名字依赖)）。`Gate<T>` 写成 `Arc<T>`，不用它就是了。
- 带配置的插件不再写工厂。`TypedPluginFactory`、`TypedFactory`，以及只为构造插件写的 `PluginFactory<C>`，换成一个构造函数 `fn new(config: &C) -> Result<Self>`，交给 `root.plugin_from(Weather::new, config)`、`builtins.register("weather", Weather::new)` 或 `export_plugin!(id: "weather", build: Weather::new)`。依赖要到运行时才知道的工厂照旧实现 `host::PluginFactory`，用 `plugin_with` 或 `Builtins::register_raw` 装。

### 错误

```rust
// 0.16
fn load(path: &Path) -> Result<String, CordisError> {
    std::fs::read_to_string(path).map_err(|e| CordisError::PluginFailed(e.into()))
}
match &error {
    CordisError::Closed => println!("closed"),
    other => println!("{other}"),
}
let shared: Arc<CordisError> = Arc::new(error);

// 0.17
fn load(path: &Path) -> rutis::Result<String> {
    Ok(std::fs::read_to_string(path)?)
}
match error.kind() {
    Some(ErrorKind::Closed) => println!("closed"),
    _ => println!("{error}"),
}
let shared: Arc<ErrorKind> = Arc::new(error.into_kind());
```

- `?` 能把任何 `std::error::Error + Send + Sync + 'static` 转成 `rutis::Error`。字符串不是错误类型：`Err("…".into())` 写成 `Err(Error::msg("…"))`，`Result<_, String>` 写成 `.map_err(Error::msg)?`。
- `err.kind()` 返回 `Option<&ErrorKind>`：来自插件代码的错误（`?` 转来的、`Error::msg`）是 `None`。`&*err` 是宿主看到的那个 `ErrorKind`，插件代码的错误在这里是 `PluginFailed`。
- 几方共同持有的错误是 `Arc<ErrorKind>`：错误汇集、`take_cleanup_errors()`、等待 `FiberView` / `dispose()` / `shutdown()` 的结果、`ErrorKind` 里的 `Arc` 成员、`LoaderError::Rejected { error }`。从 `rutis::Error` 得到它：`Arc::new(err.into_kind())`。
- thiserror：`#[from] CordisError` 写成 `#[from] rutis::Error`（`#[error(transparent)]` 照样可以）。anyhow：打开 rutis 的 `anyhow` feature，宿主里 `?` 直接转成 `anyhow::Error`，插件代码里 `.map_err(Error::from_anyhow)?`。

### 事件

```rust
// 0.16
struct Audit;
impl Listener<Created> for Audit {
    fn call<'a>(&'a self, c: &'a Ctx, e: &'a Created, _: Call<'a, String>)
        -> BoxFuture<'a, Result<Option<String>, CordisError>> {
        Box::pin(async move {
            let prefix = c.get_as::<Prefix>(TypeKey::of::<Prefix>());
            Ok(Some(format!("{} {}", prefix.map_or("", |p| p.0), e.0)))
        })
    }
}
ctx.events().on_opt(&ctx, &EventKey::<Created>::of(), Audit, EventOptions::default().prepend(true))?;
ctx.events().on_pattern(&ctx, &EventPattern::prefix("session/"), Names)?; // Names: impl PatternListener
ctx.events().emit(&ctx, &EventKey::of(), Arc::new(Created("s1".into(), 3)))?;
let out = ctx.events().waterfall(&ctx, &EventKey::of(), &Created("s1".into(), 3), ()).await?;
let quick = ctx.events().bail_sync(&ctx, &EventKey::of(), &Created("s1".into(), 3))?;

// 0.17
ctx.listen::<Created>().prepend().on(|e, call| async move {
    let prefix = call.ctx().get::<Prefix>();
    Ok(Some(format!("{} {}", prefix.map_or("", |p| p.0), e.0)))
})?;
ctx.listen::<Created>().prefix("session/").on(|_, call| async move {
    println!("{}", call.name());
    Ok(None)
})?;
ctx.emit(Created("s1".into(), 3))?;
let out = ctx.waterfall(Created("s1".into(), 3)).await?;
let quick = ctx.bail(Created("s1".into(), 3))?;
```

- 监听器只用闭包：参数是 `(Arc<E>, Call<E>)`，原来的 `&Ctx` 参数是 `call.ctx()`。`Call` 不再带生命周期，`call.next()` 消费它。闭包参数的类型由编译器推断，不用标注。
- 手写 `impl Listener<E>`、`PatternListener<E>` 的结构体（上面的 `Audit`、`Names`）：工具把它的 `call` 改成结构体自己的 `async fn on_event`，登记处改成调用它的闭包；也可以像上面那样直接写成闭包。闭包会被调用多次，要移进 `async move` 的东西先在闭包里克隆：`let audit = Arc::new(Audit); ctx.on::<Created>(move |e, call| { let audit = audit.clone(); async move { audit.run(&e, call).await } })?;`。
- 选项：`EventOptions::default().prepend(true)` / `.once(true)` / `.global(true)` 写成 `ctx.listen::<E>().prepend()` / `.once()` / `.global()`。前缀监听器从 `call.name()` 读实际的名字。
- 发出的方法收 `E` 或 `Arc<E>`，类型从值推断；`waterfall` 的终态写成 `.terminal(|e| async move { … })`，没有终态时不写；`waterfall_sync` 是宿主方法 `ctx.waterfall_sync(e, |e| …)`。
- 在另一个名字下监听：`EventKey::<E>::named(n)` / `dynamic(n)` 写成 `ctx.listen::<E>().name(n)`；`rutis_loader::edit::volatile_key(ctx)` 现在返回名字，`ctx.listen::<VolatileUpdate>().name(volatile_key(ctx)).on(…)`。
- 以运行时才定的名字发出（`EventKey::<E>::dynamic(name)` 之类）：给事件类型一个空的 `NAME`，重写 `fn name(&self) -> &str` 返回这个值的名字，再照常 `ctx.emit(value)`。工具在这里先写成 `rutis::internal::CtxInternal::named(..)` 并留下 TODO；`internal` 是 rutis 自己的 crate 用的，不承诺稳定。

```rust
#[derive(Serialize, Deserialize)]
struct Branch {
    #[serde(skip)]
    name: String, // 只用来选名字，不是参数
    step: u32,
}

impl Event for Branch {
    const NAME: &'static str = "";
    type Value = ();
    fn name(&self) -> &str {
        &self.name
    }
}

ctx.emit(Branch { name: format!("branch/{i}"), step: i })?;
```

- `EventBus` 和 `ctx.events()` 不再给插件和宿主用；上面这些 `Ctx` 方法覆盖了它们。

### 按键的服务

```rust
// 0.16
ctx.provide_as(TypeKey::of::<dyn Llm>(), Arc::new(MyLlm) as Arc<dyn Llm>)?;
ctx.provide_as(TypeKey::keyed::<dyn Llm>("openai"), openai)?;
let llm = ctx.get_as::<dyn Llm>(TypeKey::keyed::<dyn Llm>("openai"));
let notes = ctx.require_as::<Notes>(TypeKey::instance::<Notes>(id))?;
ctx.provide_as::<dyn HostDispatch>(host_key("clock"), Arc::new(Clock))?;

// 0.17
ctx.provide_as::<dyn Llm>(Arc::new(MyLlm))?;
ctx.provide_at(Key::named("openai"), openai)?;
let llm = ctx.get_at(&Key::<dyn Llm>::named("openai"));
let notes = ctx.require_at(&Key::<Notes>::of().instance(id))?;
ctx.provide_at(host_key("clock"), Arc::new(Clock))?;
```

- `host::Key<T>` 合并了原来的 `Key<T>` 和 `DepKey<T>`：`Key::of()`、`const fn Key::named("x")`、`Key::dynamic(name)`、`.instance(id)`，`.erase()` 得到 `TypeKey`。键带着值的类型，类型不符是编译错误。`provide_at`、`get_at`、`require_at`、`require` 是宿主方法（`use rutis::host::*;`）。
- rutis-bridge 返回键的函数现在返回带类型的键：`host_key(n)`、`host_key_in(n, id)` 是 `Key<dyn HostDispatch>`，`peer_key(&id)` 是 `Key<Peer>`，`transport_key(k)` 是 `Key<dyn Transport>`，`identity_key` 是 `Key<dyn Identity>`，`admission_key` 是 `Key<dyn Admission>`，`runtime_session_key` 是 `Key<dyn RuntimeSession>`，`RuntimePlugin::key(n)` 是 `Key<Runtime>`。要 `TypeKey` 的地方（`injects`、`isolate`、和 `TypeKey` 比较）写 `.erase()` 或 `.into()`。
- `ctx.get::<T>()` 现在也接受 trait 对象（`ctx.get::<dyn Llm>()`）。

### 清理

```rust
// 0.16
ctx.effect(|| {
    let task = tokio::spawn(worker(ctx.cancelled()));
    Effect::AsyncDisposer(Box::new(move || Box::pin(async move {
        let _ = task.await;
        Ok(())
    })))
})?;

// 0.17
ctx.effect(|| {
    let task = tokio::spawn(worker(ctx.cancelled()));
    async move {
        let _ = task.await;
        Ok(())
    }
})?;
```

闭包照旧立即执行，返回的 future 是清理；插件停下时登记被拒、闭包不执行的规则不变。`Effect::Disposer`（同步的清理）也写成 `async move { …; Ok(()) }`；`Effect::Many` 写成多次 `effect`；`Effect::Done` 就是不调用 `effect`。`Effect` 本身还在 `rutis::host` 里，`DynPlugin::apply` 照旧返回它。

### 宿主

```rust
// 0.16
use rutis::{CordisError, Ctx, FiberState, TypeKey, Typed};

// 0.17
use rutis::host::*;                // Ctx::root、shutdown、require、FiberState、TypeKey…
use rutis::Ctx;
```

- `Ctx::root`、`root_with`、`root_with_sink`、`shutdown`、`shutdown_with_timeout`、`diagnostics`、`isolate`、`scope`、`plugin_with`、`plugin_from`、`require`、`take_cleanup_errors` 等宿主方法来自扩展 trait `host::CtxHost`，`use rutis::host::*;` 之后照旧在 `Ctx` 上调用。
- `FiberView`、`FiberState`、`Snapshot`、`PluginId`、`TypeKey`、`InstanceId`、`ErrorSink`、`default_sink`、`CallError`、`BoxFuture`、`Effect` 等在 `rutis::host`；诊断类型在 `rutis::host::diagnostics`；`Mode`、`ListenerInfo`、`Scope`、`Filtered` 在 `rutis::events`。
- `ctx.plugin(…)` 只有一个，装任何插件：`Plugin`、`DynPlugin`，以及 `Box<dyn DynPlugin>`。

### rutis-loader

根上只留下组装加载器的 11 个名字：`LoaderPlugin`、`LoaderOptions`、`Loader`、`Layer`、`Patch`、`Builtins`、`Chain`、`Resolver`、`ServiceCatalog`、`LoaderError`、`RowEvent`。其余按主题放进模块：

| 模块 | 内容 |
| --- | --- |
| `report` | `CheckReport`、`ReconcileReport`、`RowInfo`、`RowStatus`、`RowState`、`RowCheck`、`EntryInfo`、`EntryStatus`、`Failure`、`Unavailable`、`InstanceInfo`、`LoaderChanged` 等 |
| `edit` | `Edit`、`Editable`、`NewEntry`、`CreateInstance`、`Persist`、`NoPersist`、`PersistError`、`Version`、`apply_edit`、`VolatileUpdate`、`volatile_key`、`volatile_paths` 等 |
| `resolve` | `Build`、`Resolved`、`Declarations`、`Declared`、`ScopedFactory`、`Expressions`、`ExprScope` |
| `runtime` | `RuntimeResolver`、`RuntimeRowsPlugin`、`RuntimeRows` 等；`runtime::go`、`runtime::swift`、`runtime::binary` 是各语言的 `*Resolver`、`*Runtimes`、`RestartPolicy` 等 |
| `peer` | `PeerResolver`、`register_peer_node`、`PeerRows`、`PeerRowsPlugin`、`LoaderCatalog` |

```rust
// 0.16
builtins.register("weather", WeatherFactory);                     // impl PluginFactory<WeatherConfig>
builtins.register_fn::<Value, _>("echo", |_| Ok(Box::new(Echo)));
builtins.register_with("notes", |build| Ok(NotesFactory::new(build.instance("session")?)));

// 0.17
builtins.register("weather", Weather::new);                       // fn new(&WeatherConfig) -> Result<Weather>
builtins.register::<Value, _, _>("echo", |_| Ok(Echo));
builtins.register_with("notes", |build: &Build, config: &NotesConfig| {
    Notes::new(build.instance("session")?, config)
});
```

- `register(name, build)`：`build` 是 `Fn(&C) -> Result<P>`，`P: Plugin`；配置的 Schema 从 `C` 生成（`C` 是 `serde_json::Value` 时没有），依赖从 `P::Deps` 得出。闭包参数没写类型时，用 `register::<C, _, _>` 写明 `C`。
- `register_fn` 删除，用 `register`。`register_raw(name, factory, schema)` 照旧接受 `PluginFactory<Value>`，给依赖要到运行时才知道的插件用。
- `register_with(name, |build, config| …)` 现在每份直接构造插件。原来按实例构造的工厂，如果它的依赖要到运行时才知道（例如键里带着实例），写一个自己的 `Resolver`，返回 `Resolved::new(factory).with_scoped(scoped)`，`scoped` 从 `Build` 为每份构造工厂，再放进 `Chain`：

```rust
use std::sync::Arc;
use rutis::host::{BoxFuture, PluginFactory};
use rutis_loader::resolve::{Build, Resolved, ScopedFactory};
use rutis_loader::{LoaderError, Resolver};
use serde_json::Value;

struct NotesResolver;

impl Resolver for NotesResolver {
    fn resolve<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Arc<Resolved>, LoaderError>> {
        Box::pin(async move {
            if name != "notes" {
                return Err(LoaderError::NotFound { name: name.to_owned() });
            }
            let scoped: ScopedFactory = Arc::new(|build: &Build| {
                let factory: Arc<dyn PluginFactory<Value>> = Arc::new(NotesFactory::new(build.instance("session")?));
                Ok(factory)
            });
            Ok(Arc::new(Resolved::new(Arc::new(OutsideInstances)).with_scoped(scoped)))
        })
    }
}
```

`OutsideInstances` 是实例之外用的工厂（`build` 返回一个错误即可）。

### rutis-sdk（dylib 插件）

```rust
// 0.16
rutis_sdk::export_plugin! { id: "weather", factory: WeatherFactory }

// 0.17
rutis_sdk::export_plugin! { id: "weather", build: Weather::new }
```

`build:` 接受和 `Builtins::register` 相同的构造函数，配置从 JSON 解码成它的参数类型；`schema:` 照旧可选。`factory:` 保留，给依赖要到运行时才知道的 `PluginFactory<ConfigValue>` 用。dylib 插件本来就要用和宿主相同的 SDK 构建，0.17 的插件照常重新构建。

## 没有变化的

- 门控、重载、卸载、清理、事件的五种方式、作用域、过滤，以及[保证](guarantees.md)里的每一条。
- 协议和线上格式；Rust 服务抛出的 rutis 错误在其他语言里仍叫 `CordisError`。
- 其他语言的 SDK、运行时、`rutis.json` 和 rutis-host。
