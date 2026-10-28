# Rutis 开发手册

本手册介绍插件的实现、服务调用、资源管理和运行控制。应用结构的设计方法见 [应用开发指南](development-guide.md)。

## 快速开始

在 Rust 项目中添加依赖：

```toml
[dependencies]
rutis = "0.17"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }
serde = { version = "1", features = ["derive"] }
```

下面用一个提供方和一个消费者演示服务注册与依赖声明。示例中，Backend 提供服务，Indexer 声明并使用这个服务。

```rust
use std::sync::Arc;
use rutis::host::*;
use rutis::{Ctx, Plugin};

struct Backend(String);
struct BackendPlugin(String);

impl Plugin for BackendPlugin {
    type Deps = ();

    fn name(&self) -> &str { "backend" }

    async fn apply(&self, ctx: &Ctx, (): ()) -> rutis::Result {
        ctx.provide(Backend(self.0.clone()))?;
        Ok(())
    }
}

struct IndexerPlugin;

impl Plugin for IndexerPlugin {
    type Deps = Arc<Backend>;

    fn name(&self) -> &str { "indexer" }

    async fn apply(&self, _: &Ctx, backend: Arc<Backend>) -> rutis::Result {
        println!("Indexer connected to {}", backend.0);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Ctx::root()?;
    let started = async {
        let backend = root.plugin(BackendPlugin("local".into()));
        (&backend).await?;

        let indexer = root.plugin(IndexerPlugin);
        (&indexer).await
    }.await;

    let closed = root.shutdown().await;
    started?;
    closed?;
    Ok(())
}
```

运行后输出 `Indexer connected to local`。Indexer 的 `type Deps = Arc<Backend>` 既是依赖声明，也是 `apply` 收到的参数。这个例子先完成后端初始化，再加载 Indexer。实际应用也可以先注册消费者：它会在 `Pending` 状态等待依赖就绪。`Ctx::root()`、`shutdown()` 等宿主用的方法来自 `use rutis::host::*;`。

这里的 `(&view).await` 等待该插件处理完此前排队的操作。判断一个运行中的服务是否可用，应观察提供它的插件是否进入 `Active`，具体见 [启动与关闭](#启动与关闭)。

仓库中的 [完整索引示例](../crates/rutis/examples/development_workflow.rs) 在此基础上加入了有界队列、后台任务和配置更新：

```bash
cargo run -p rutis --example development_workflow
```

## 实现插件

### 初始化

`apply` 按以下顺序组织：

1. 取得本次加载需要的服务：必需的写在 `type Deps` 里，作为参数传入。
2. 创建资源，并用 `ctx.effect` 登记对应的清理。
3. 提供服务、注册监听器或创建子插件。
4. 返回 `Ok(())`，完成初始化。

插件进入 `Active` 后，其服务即可供外部消费者使用。持续运行的工作放在后台任务中，由清理负责结束。

`type Deps` 声明服务依赖，没有依赖时写 `()`；`name()` 用于日志和诊断，默认是类型名的最后一段；`validate()` 在 `apply` 之前校验插件持有的配置。插件实现需要满足 `Send + Sync + 'static`。

### 选择创建方式

| 创建方式 | 适用情况 | 重新加载时的行为 |
|---|---|---|
| `ctx.plugin(plugin)` | 配置由插件对象持有 | 在同一个对象上再次执行 `apply` |
| `ctx.plugin_with(factory, config)` | 运行期间需要更新配置（实现 `host::PluginFactory`） | 调用工厂的 `build` 从当前配置创建新的插件对象 |

插件的依赖写在 `type Deps` 中；工厂写在 `PluginFactory::injects()` 中；按名字依赖服务的插件实现 `host::DynPlugin`，写在 `DynPlugin::injects()` 中。声明在注册时确定，并在该 fiber 的整个生命周期内保持固定。

创建资源的代码放在 `apply` 中。这样，每次重新加载都会取得当前依赖，创建所需资源，并登记新的清理函数。

## 服务键与作用域

### 定义服务键

注册、依赖声明和读取使用同一个键：

| 服务形式 | 注册 | 依赖声明 | 读取 |
|---|---|---|---|
| 具体类型 | `provide(value)` | `type Deps = Arc<T>` | `apply` 的参数；可选读取 `get::<T>()` |
| trait 接口 | `provide_as::<dyn T>(value)` | `type Deps = Arc<dyn T>` | `apply` 的参数；可选读取 `get::<dyn T>()` |
| 带名称的服务 | `provide_at(key, value)` | `DynPlugin::injects()` 里的 `key.erase()` | `require_at(&key)`；可选读取 `get_at(&key)` |

其中，`provide_as` 和 `provide_at` 接收 `Arc<T>`。trait 服务按 trait 对象的类型登记；例如，`provide_as::<dyn Store>(Arc::new(store))` 提供的就是 `Arc<dyn Store>` 依赖的那个服务。按键读写的 `provide_at`、`get_at`、`require_at` 是宿主层的方法，`use rutis::host::*;` 之后可用。

同类型的多个服务通过名称区分，键 `host::Key<T>` 带着服务的类型：

```rust
use rutis::host::Key;

struct Backend;

const PRIMARY_BACKEND: Key<Backend> = Key::named("primary");
```

把这类键放在公共接口模块，供提供方和使用方共同使用。运行时生成的名称使用 `Key::dynamic`。

### 声明依赖

把插件运行必需的服务写进 `type Deps`，多个时写成元组。框架会等待这些服务存在、提供方进入 `Active`，以及可选的健康检查通过，然后执行 `apply`。

`get` / `get_at` 负责查找当前可见的服务，返回 `Option<Arc<T>>`；它不等待服务，也不建立重载关系，诊断里记为未声明的访问。自动重载关系由依赖声明（`Deps`，或 `DynPlugin::injects()`）建立，因此每个必需服务都应写进依赖声明。`Deps` 里的 `Option<Arc<T>>` 是装载时的快照：不等待，之后出现或消失都不重载。

带额外健康条件的服务（同步谓词，条件改变后重新检查依赖，据此调整消费者状态）由 `rutis::internal` 里的 `provide_checked` 和 `refresh` 提供，只给 rutis 自己的 crate 用，不承诺稳定。

### 选择作用域

按类型提供的服务在 root 内共享。同一个键和作用域对应一个有效服务绑定；重复提供会返回 `ServiceExists`。

`isolate` 为指定服务键选择作用域。下面的片段为同一个键建立两个独立的查找范围：

```rust,ignore
let scope_a = root.isolate(PRIMARY_BACKEND, "a");
let scope_b = root.isolate(PRIMARY_BACKEND, "b");
```

在对应上下文下注册提供方和消费者，即可分别使用各自的绑定。同一个 root 内，同键同 label 共享作用域。isolate 作用于指定服务键，资源生命周期沿用原上下文；实例级资源管理见 [实例子树](#实例子树)。

## 管理资源

插件作者负责业务资源的完整生命周期，通过 `apply` 实现初始化，通过 `ctx.effect` 登记清理。Rutis 按生命周期规则调用这些实现。职责说明见 [职责划分](development-guide.md#职责划分)。

### 登记清理

通过 `ctx.provide` 注册的服务、通过当前 `ctx` 注册的监听器，以及 `ctx.plugin` 创建的子插件，都由框架登记清理。外部订阅、连接和任务通过 `ctx.effect` 登记释放过程。

以下片段放在插件的 `apply` 中，演示异步清理的写法：

```rust,ignore
ctx.effect(move || async move {
    connection.close().await?;
    Ok(())
})?;
```

`effect` 的闭包立即执行，它返回的 future 就是清理，在卸载时执行。插件已经在停下时，`effect` 返回 `InactiveEffect`，闭包不执行。资源取得后应及时登记清理，同时处理登记失败时的释放。`apply` 中途失败时，框架会执行已经登记的清理。

清理句柄 `Disposer` 可用于提前释放：调用 `disposer.dispose().await`，等待并处理结果。保留或丢弃句柄，都允许所属插件在卸载时负责最终清理。

### 管理后台任务

后台任务需要同时安排取消和等待退出。下面的片段展示了基本结构；任务中的等待可替换为业务循环：

```rust,ignore
let stopping = ctx.cancelled();

ctx.effect(move || {
    let task = tokio::spawn(async move {
        stopping.await;
    });
    async move {
        task.await?;
        Ok(())
    }
})?;
```

每次 `apply` 取一次当前这一代的 `ctx.cancelled()`，交给这一代的任务；插件开始停下时它就完成，早于清理。任务收到这个信号后，按约定完成或取消当前操作，然后退出。清理通过 `JoinHandle` 等待实际退出。用 `Disposer` 提前释放这个清理时插件还没有停下，任务需要另外的停止方式。

业务循环通常用 `tokio::select!` 同时等待输入和取消。队列中的请求可以选择完成、取消或保存，具体策略由服务接口约定。完整实现见 [IndexerPlugin 示例](../crates/rutis/examples/development_workflow.rs)。

同步计算、阻塞操作和子进程也需要各自的停止方式，例如分段检查取消、关闭 I/O 或结束进程并等待退出。

### 安排清理顺序

同一插件内的清理按登记顺序逆序执行，后登记的资源先释放。（`host::DynPlugin` 的 `apply` 返回的 `Effect` 最后登记。）

当底层连接需要保留到消费者退出时，采用以下顺序：

```text
初始化：创建连接并登记关闭 → 提供服务 → 创建依赖它的子插件
清理：  关闭子插件 → 摘除服务并等待消费者退出 → 关闭连接
```

清理中需要使用的对象，在 `apply` 中取得并捕获。一个清理函数失败后，框架会记录错误并继续执行其余清理。

服务从注册表移除后，调用方持有的 `Arc` 仍然有效。服务实现应根据自身运行状态处理后续调用。例如，示例中的旧 Indexer 会在队列关闭后返回错误。

## 使用事件

### 事件总线

事件总线管理监听器的注册、事件的分发和处理函数的调用。发送方提供事件数据，监听器实现具体处理逻辑，分发 API 决定执行顺序、完成条件和结果返回方式。

这种机制通常用于通知、请求分发或多个插件共同参与的处理过程。事件类型定义数据和结果，应用据此约定具体含义。

### 定义事件和监听器

定义事件类型时，实现 `Event`，指定事件名称和处理结果类型；事件和结果类型实现 serde 的 `Serialize` 与 `Deserialize`（依赖中加入 `serde = { version = "1", features = ["derive"] }`）。监听器是一个闭包，参数是事件（`Arc<E>`）和这次调用（`Call<E>`）。下面以携带文档名称的 `DocumentIndexed` 为例：

```rust
use rutis::{Ctx, Event};

#[derive(serde::Serialize, serde::Deserialize)]
struct DocumentIndexed(String);

impl Event for DocumentIndexed {
    const NAME: &'static str = "app/document-indexed";
    type Value = ();
}

fn index(ctx: &Ctx, path: String) -> rutis::Result {
    ctx.on::<DocumentIndexed>(|event, _call| async move {
        println!("indexed: {}", event.0);
        Ok(None)
    })?;
    ctx.emit(DocumentIndexed(path))?;
    Ok(())
}
```

在这个例子中，`on::<DocumentIndexed>` 为名称 `DocumentIndexed::NAME` 注册监听器，`emit` 以这个名称发送事件；两者都返回 `Result`，失败时（例如上下文已经关闭）不会注册或投递。闭包的参数类型由编译器推断。

事件按名称匹配。`ctx.listen::<E>().name("…")` 在另一个名称下监听同一事件类型，`ctx.listen::<E>().prefix("…")` 监听一个前缀下的所有名称（`call.name()` 是实际的名称）；没有固定名称的事件类型重写 `Event::name`，让每个值带上自己的名称。不同 Rust 类型使用同一名称时听到同一批事件，框架经 serde 转换，所以不相关的事件类型应使用不同名称。以 `rutis/` 开头的名称留给框架事件，插件只能监听。

监听器属于注册它的 `Ctx`，回调里的 `call.ctx()` 就是这个 `Ctx`，而不是发送方的。回调通过它注册的资源因此属于监听插件，`ctx.cancelled()` 就是它所在插件的卸载信号。插件开始卸载后，它的监听器不再被调用，卸载会等待它们正在运行的调用结束。可运行代码见 [上下文归属示例](../crates/rutis/examples/listener_ctx_ownership.rs)。

### 选择分发方式

监听器只注册一次，每种分发方式都会调用它；由发送方选择分发方式，回调可以用 `call.mode()` 查看本次是哪一种。

| API | 执行方式 | 返回与错误处理 |
|---|---|---|
| `emit` | 在后台按注册顺序启动各监听器的调用，同名事件按发送顺序启动，调用之间并发执行 | 立即返回；每个失败以 `ListenerFailed` 交给 ErrorSink，不影响其他监听器 |
| `parallel` | 并发调用监听器，等待全部完成 | 成功返回 `()`；失败汇总为 `ListenersFailed`（只有一个失败时也是），按注册顺序排列 |
| `serial` | 按注册顺序逐个调用，遇到结果或错误时结束 | 返回首个结果；`None` 和 `Some(false)` 视为没有结果 |
| `waterfall` | 监听器通过 `call.next()` 调用后续处理，并可处理其返回结果 | 返回最外层监听器的结果 |
| `bail` | 与 `serial` 相同，但在调用方线程内同步执行 | 监听器需用 `on_sync` 注册；遇到异步监听器返回 `AsyncListener` 错误 |

监听器的返回类型为 `rutis::Result<Option<E::Value>>`。例如，事件定义 `type Value = String`，serial 监听器就用 `Ok(Some(title))` 返回标题，用 `Ok(None)` 交给下一位处理者。

在 waterfall 中，监听器调用 `call.next().await` 执行后续流程，并可以处理返回结果；直接返回则结束当前流程。其他分发方式中调用 `next` 得到 `WaterfallNext` 错误。调用方用 `ctx.waterfall(event).terminal(…)` 提供链末的处理函数，可以省略。事件参数在整条链中保持相同，结果通过返回值传递。

emit 和 parallel 忽略监听器返回的 `Some(value)`；serial 用它决定何时结束调用。选择分发方式时，应明确是否等待完成、是否需要返回值，以及处理函数之间的顺序要求。

慢监听器不会挡住同名事件的其他调用，但每次 emit 会一直留在积压中，直到它启动的调用全部结束。`ctx.diagnostics().event_backlogs` 列出各事件名已接收未完成的 emit 数量和最早一个的等待时长，可据此发现慢监听器。高频数据流可以使用有界队列，结合批处理或合并控制积压；需要重放的数据另行持久化。处理错误通过 `Result` 表达，waterfall 中的 panic 由调用方处理。

## 实例子树

实例子树把一个插件及其后代组织成共同管理的范围。实例服务键限制服务的访问范围；事件按名字送达所有监听器，实例信息只作过滤描述；`FiberView::shutdown()` 负责关闭整棵子树。

这种组织方式可以用于管理多个生命周期独立的运行实例。下面以工作区为例演示服务与事件的关联方式。

### 示例：共享实例状态

工作区父插件用自己的实例 ID 创建服务键，再把键传给子插件。以下片段放在 `WorkspacePlugin::apply` 中，业务类型由应用定义：

```rust,ignore
let workspace = ctx.instance();
let state_key = Key::<WorkspaceState>::of().instance(workspace);

ctx.provide_at(state_key.clone(), Arc::new(state))?;
ctx.plugin(IndexerPlugin::new(state_key));
Ok(())
```

Indexer 实现 `host::DynPlugin`：传入的键在 `injects()` 里写成 `state_key.erase()`，在 `apply` 里用 `require_at(&state_key)` 读取。该服务的访问范围是工作区及其子插件；外部管理功能通过工作区显式提供的业务接口调用。

父插件提供状态、创建子插件后即完成初始化。宿主随后等待 Indexer 就绪，这样子插件可以顺利取得已经 Active 的父插件服务。

### 示例：实例内的事件分发

Progress 和 Indexer 作为工作区插件的子插件，各自使用自己的上下文。用 rutis-loader 的实例组时，加载器把每个组实例标记为一个作用域：

```rust,ignore
// 在 Progress 子插件中注册。
progress_ctx.on::<DocumentIndexed>(|event, _call| async move {
    println!("indexed: {}", event.0);
    Ok(None)
})?;

// 在 Indexer 子插件中发送。
indexer_ctx.emit(DocumentIndexed(path))?;
```

事件是平总线：一个 root 里按名字送达所有监听器，与发出方或监听器的作用域无关。作用域只是 `ListenerInfo` 里的描述，供过滤使用；要按实例筛选，用 `filter` 或不同的事件名。关闭子树时，框架等待其中监听器正在运行的调用结束，再完成资源清理。回调中产生的额外后台任务，按前面的任务管理方式登记。

isolate 的服务作用域独立于事件作用域。

关闭操作由宿主协调。实例回调若需要关闭工作区，可向宿主发送请求并返回，由宿主等待关闭完成。这样，关闭过程可以正常等待当前回调结束。

## 更新配置

需要热更新的插件由一个构造函数从配置构造。下面的构造函数复用快速开始中的 `BackendPlugin`，并在构造时检查配置：

```rust,ignore
use rutis::host::ErrorKind;

fn backend(label: &String) -> rutis::Result<BackendPlugin> {
    if label.trim().is_empty() {
        return Err(ErrorKind::Validation {
            issues: vec!["backend label is required".into()],
        }
        .into());
    }
    Ok(BackendPlugin(label.clone()))
}
```

宿主实现 `host::PluginFactory`，在 `build` 里调用构造函数，用 `plugin_with` 注册，之后提交新配置：

```rust,ignore
struct BackendFactory;
impl PluginFactory<String> for BackendFactory {
    fn build(&self, label: &String) -> rutis::Result<Box<dyn DynPlugin>> {
        Ok(Box::new(backend(label)?))
    }
}

let backend = root.plugin_with(BackendFactory, String::from("local"));
backend.update(String::from("remote")).await?;
```

一次更新分为两步：

1. **预检查。** 执行工厂的 `validate_config`、`build` 和插件的 `validate`。失败时返回错误，现有配置和实例保持原状。
2. **重新加载。** 保存新配置，清理旧实例，依赖就绪后再次构造和加载插件。消费者随服务变化重新加载。

构造函数（以及工厂的 `build`）只负责构造对象，网络连接、任务启动等操作放在 `apply` 中。首次加载也需要的校验写在构造函数或插件的 `validate` 中；工厂的 `validate_config` 用于更新预检查。

正式加载仍可能遇到网络或资源错误，宿主应保留恢复策略，例如重试或重新提交上次有效配置。更新后观察实际业务入口的状态，再恢复请求处理。需要改用另一组依赖时，由宿主选择并注册相应插件。

`current_config::<C>()` 返回保存的配置快照，可用于展示和诊断。配置更新作用于当前进程内的插件实例；代码升级由应用发布流程负责。

## 启动与关闭

### 观察运行状态

| 状态 | 含义 |
|---|---|
| `Pending` | 等待依赖服务就绪 |
| `Loading` | 正在初始化 |
| `Active` | 初始化成功，服务可用 |
| `Failed` | 初始化失败，错误保存在状态快照中 |
| `Unloading` | 正在清理资源 |
| `Disposed` | 当前插件已卸载 |

用 `view.state()` 读取快照，用 `view.watch()` 等待变化。启动等待应处理 Active、Failed、Disposed 和超时，完整函数见 [示例中的 wait_active](../crates/rutis/examples/development_workflow.rs)。

`(&view).await` 表示该插件此前排队的操作已处理完毕。等待结束时，插件仍可能因缺少依赖而处于 `Pending`；业务入口应在所需插件进入 `Active` 后开放。后台任务的健康状况由服务状态和错误报告反映。

### 选择关闭范围

关闭一个插件及其子树，使用该插件的 `FiberView::shutdown()`；关闭整个应用，使用 `Ctx::shutdown()`。任何子上下文上的 `Ctx::shutdown()` 也会关闭整个 root。

`view.restart()` 清理并重新加载当前插件。`view.dispose()` 卸载插件，普通插件到此结束；需要再次使用时注册新实例。root 的普通 dispose 支持后续 restart，最终退出应用使用 shutdown。

### 设置等待时间

`dispose_with_timeout` 和 root 的 `shutdown_with_timeout` 提供有限等待。超时后，关闭过程继续运行，调用方可以记录状态并再次等待同一结果。

对子树关闭设置等待时间，可以使用 Tokio timeout 包装 `view.shutdown()`：

```rust
use std::sync::Arc;
use std::time::Duration;
use rutis::host::{ErrorKind, FiberView};

async fn try_shutdown(view: &FiberView, limit: Duration)
    -> Result<bool, Arc<ErrorKind>>
{
    match tokio::time::timeout(limit, view.shutdown()).await {
        Ok(result) => {
            result?;
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}
```

返回 `true` 表示关闭完成；`false` 表示本次等待超时。宿主保留该 view，稍后再次调用 `view.shutdown().await` 取得结果。关闭操作一旦发起，就独立于等待方继续执行。

插件卸载时，框架等待它的监听器正在运行的调用结束（它们看到 `ctx.cancelled()`）。应用需要等待的其他工作，通过任务句柄和清理函数明确管理。

## 诊断与验证

### 查看问题

`root.diagnostics()` 提供插件状态、依赖和服务绑定的快照。可以从以下信息开始排查：

| 现象 | 检查内容 |
|---|---|
| 插件保持 Pending | 缺失的键、服务作用域、提供方状态、健康检查结果 |
| 服务读取返回 None | 注册与读取的类型、限定名、实例 ID，以及提供方是否 Active |
| 更新后仍使用旧服务 | 依赖声明是否完整，使用方是否取得新一代对象 |
| 插件进入 Failed | `view.state().error` 中的具体原因 |
| 关闭持续等待 | 初始化是否结束，任务是否响应取消，清理和监听器回调是否完成 |

`diagnostics()` 读取已有状态。使用健康谓词的服务（见[声明依赖](#声明依赖)）在条件变化后先调用 `refresh()`，再观察检查结果。

### 处理错误

生命周期操作和等待型事件的错误通过返回值处理。错误类型是 `rutis::Error`，`?` 能把任何标准错误转成它；要区分 rutis 自己的失败时用 `err.kind()`，它返回 `Option<&ErrorKind>`，来自插件代码的错误是 `None`。emit 中监听器的错误（以 `ListenerFailed` 包装）以及重载期间的清理错误进入 ErrorSink；默认输出到 stderr，宿主可用 `Ctx::root_with_sink` 接入自己的日志。ErrorSink，以及等待 `FiberView`、`dispose()`、`shutdown()` 得到的错误，是共享的 `Arc<ErrorKind>`。

提前释放 effect 时，先处理 `Disposer::dispose()` 的结果；同一个错误之后在终态卸载时进入结果，在重载时交给 ErrorSink。后台任务的错误由任务所有者观察和报告。

### 验证关键过程

为插件验证以下场景：

- 依赖稍后出现时，插件从 Pending 进入 Active，并正常提供服务。
- 依赖更新后，旧任务退出，新实例使用新服务，长期状态按设计保留。
- 初始化中途失败时，已创建的资源得到清理。
- 配置预检查失败时，原实例继续工作；正式加载失败时，可以执行恢复策略。
- 关闭一个实例子树后，其他独立子树继续运行。
- 在任务执行期间关闭，能取得约定的请求结果和最终清理结果。

测试中使用状态、oneshot、Notify 或 Barrier 协调时序，并为等待设置超时。仓库内可以运行：

```bash
cargo test -p rutis
cargo run -p rutis --example development_workflow
```

进一步的行为细节见 [生命周期契约测试](../crates/rutis/tests/contract.rs)、[配置更新测试](../crates/rutis/tests/config_update.rs)、[实例子树测试](../crates/rutis/tests/instance_subtrees.rs)和[关闭与等待时间说明](core-shutdown-and-disposal-deadline.md)。

---

适用于 Rutis 0.17。标注为片段的代码在相应上下文中使用；完整程序见快速开始和配套示例。
