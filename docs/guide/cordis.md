# Cordis

rutis 和 Cordis（Node 插件框架）有两种互通方式：

- **Cordis 应用作为节点**：已有的 Cordis 应用接到 rutis 节点上，互相使用服务、共享事件，并让 rutis 在它里面运行插件。用 `@arcships/rutis-runtime/bridge`。
- **在 Rust 应用里挂载 Cordis 插件**：已发布的 Cordis 插件在真实的 Cordis 中运行，源码不改，Rust 侧拿到构建时从它的类型声明生成的 Rust 类型。用 `rutis-bridge` 的 `cordis` feature。

此外，Cordis 插件本身也可以作为 rutis-loader 的行运行，`rutis.json` 里写包名就行，包不用改，见 [写一个 TypeScript 插件](typescript-plugin.md#已有的-cordis-插件)：

- 同一个 Node 运行时里的 Cordis 行按 Cordis 的原生规则互相使用服务，拿到的是对象本身。
- 一行提供的服务自动投到 rutis，别的运行时的行、别的节点和 Rust 插件按名字使用；方法取服务对象上的函数（自身和原型链上的，不含 `constructor`），都按 `async` 调用。包的 `package.json` 里写了 `rutis.provides` 时按它（例如把某些方法标成 `sync`）。
- 同名服务已经有别的提供者（另一个运行时的行、Rust 插件、节点导入）时，后提供的那一行失败（装载后才提供的也一样），错误写明双方，例如 `service storage is already provided by row prompts: row storage cannot provide it too`。
- `rutis-host check` 不运行插件，看不到没有声明的服务：一行依赖的服务只可能由这类行提供时，写一条告警 `nothing declares <服务>; it may be provided at runtime by …`，不算失败（`check --strict` 时算）。

## 事件

Cordis 插件照常用 Cordis 自己的事件 API，它就是 rutis 的事件 API：`ctx.on`、`ctx.emit`、`ctx.parallel`、`ctx.serial`、`ctx.waterfall`、`ctx.bail`。Cordis 的监听器自动登记到宿主的逻辑总线上，所以不论 Cordis 插件作为 rutis.json 的行运行、挂载在 Rust 应用里，还是在作为节点的 Cordis 应用里，它的事件都按同一套规则到达任何语言、任何进程里的监听器（平总线、按名字），别处发出的事件也到达它的监听器。作为节点的 Cordis 应用只与对端共享 `Events` 的 `export` / `import` 列出的名字（见[下文](#cordis-应用作为节点)）。规则见 [插件 API：事件](plugin-api.md#事件)。

```ts
export function apply(ctx: Context) {
  // waterfall：最后一个参数是 next，别处（Python、Go、Rust）的监听器也在这条链上
  ctx.on('chat/before-send', async (message: string, next: () => Promise<string>) => (await next()).trim())

  ctx.on('chat/sent', (message: string) => { /* 出错不影响发出方 */ })
}

// 插件里的任何地方发出：
const out = await ctx.waterfall('chat/before-send', text, (text: string) => text)  // 最后一个参数是终态
await ctx.parallel('chat/sent', out)       // 等每个监听器，不论在哪里
```

- **过滤**：Cordis 的 `Context.filter` 接到 rutis 的过滤上。`ctx.emit(thisArg, name, ...)`（以及其余四种）的 `thisArg[Context.filter]` 是函数时用它过滤：本进程的监听器传入它自己的 Context，别处的监听器传入 `{ rutis: 描述 }`。每个 Context 都有只读的 `ctx.rutis`，所以 `c => c.rutis.row === 'chat'` 对两者都有效。描述的字段见[过滤](plugin-api.md#过滤)。
- **`prepend` / `global`**：就是 Cordis 自己的监听器选项，对别处的监听器同样排序、同样豁免过滤。
- **同步 `bail`**：别处的监听器作为同步调用执行；遇到 `async` 函数或返回 Promise 的监听器以 `AsyncListener` 失败（以前 Cordis 把 Promise 当作结果）。
- **`parallel`** 的失败总是 `AggregateError`，子错误按登记顺序（与 Cordis 相同），信息为 `<n> listeners of <name> failed`，别处监听器的失败也在其中。`emit` 的监听器出错只在它所在的一侧报告一行，不影响发出方。
- **`internal/*`** 仍是每个运行时自己的：`internal/dispatch`、`internal/service` 只在本进程里，从不跨进程。从别处到达的事件第一次调用本进程的监听器之前，运行时对这次分发触发一次 `internal/dispatch`，方式是 rutis 的方式名（`emit`、`parallel`、`serial`、`waterfall`、`bail`），所以它的校验插件照常看得到它们。
- **卸载**：这一行的 `ctx.signal` 被中止，正在运行的监听器被等待，然后才运行清理。

## Cordis 应用作为节点

```bash
npm install @arcships/rutis-runtime
```

```js
import { Link, Export, Import, Host, Events } from '@arcships/rutis-runtime/bridge'

ctx.plugin(Link, { peer: 'main', id: 'mac', dial: 'wss://main.example.com/rutis', token: process.env.RUTIS_TOKEN })  // 或 listen: 'wss://…'
ctx.plugin(Export, { peer: 'main', services: { calendar: { today: 'sync', later: 'async' } } })
ctx.plugin(Import, { peer: 'main', services: ['clock'] })
ctx.plugin(Host, { peer: 'main' })                     // 允许 main 在这里运行装着的 npm 插件
ctx.plugin(Events, { peer: 'main', export: ['tock'], import: ['session/*'] })
```

- `Link` 维持与一个对端的连接，把它作为服务 `rutisPeer.<id>` 提供；其他功能依赖它，所以每次重连都会重新建立。
- `Export` 把 Cordis 服务按声明的方法形状给对端；`Import` 把对端的服务注册为 Cordis 服务（本地已有同名服务时拒绝）。
- `Host` 让对端在这里加载已安装的 npm 插件，并按行的 `isolate` / `inject` 运行；只对可信的对端打开。
- `Events` 与对端共享事件，规则与节点之间相同（[节点之间的事件](nodes.md#节点之间的事件)）：`export` 里的名字，本应用发出的事件到达对端的监听器；`import` 里的名字，本应用的监听器收到对端发出的事件。一项是名字，或以 `*` 结尾的前缀。五种方式、过滤与全部值类型都可用；只走一跳。0.13 之前的 `out` / `in` 写了会报错。
- 凭据也可以来自环境：`RUTIS_TOKEN`、`RUTIS_CA`、`RUTIS_CERT`、`RUTIS_KEY`。

对端（rutis 节点）的配置见 [连接节点](nodes.md)。

## 在 Rust 应用里挂载 Cordis 插件

当前支持 Unix（Linux、macOS），需要 Node 22 或更高。设计与边界见 [兼容层设计](../design-protocol-plugin-mount.md)，完整示例见 [`examples/dsh-baseline`](../../examples/dsh-baseline)。

### 1. 准备 npm 项目

在应用旁边准备一个 npm 项目，安装要挂载的插件和运行时 [`@arcships/rutis-runtime`](https://www.npmjs.com/package/@arcships/rutis-runtime)（源码在仓库的 `node/rutis-runtime`）。版本以这个项目的锁文件为准；运行时的协议版本须与 crate 一致（构建时检查）：

```json
{
  "private": true,
  "type": "module",
  "dependencies": {
    "@deepseek-ai/cordis": "4.0.4",
    "@deepseek-ai/dsh-credentials-local": "0.2.0-rc.1",
    "@arcships/rutis-runtime": "0.17.2"
  }
}
```

```sh
npm --prefix cordis ci
```

构建不会自动安装 npm 依赖；缺少包时构建失败，并给出要执行的命令。在本仓库内开发时可改用 `"file:../path/to/rutis/node/rutis-runtime"` 引用源码。

### 2. 在 Cargo.toml 中声明挂载

```toml
[dependencies]
rutis = "…"
rutis-bridge = { version = "0.17", features = ["cordis"] }
tokio = { version = "1", features = ["full"] }

[build-dependencies]
rutis-bridge = { version = "0.17", features = ["cordis"] }

[package.metadata.rutis-cordis]
npm = "cordis"                   # 第 1 步的 npm 项目，相对 Cargo.toml
# runtime = "…"                  # 默认 <npm>/node_modules/@arcships/rutis-runtime

# 一个挂载 = 一个生成的模块
[package.metadata.rutis-cordis.mounts.credentials]
plugin = "@deepseek-ai/dsh-credentials-local"   # npm 包；或 path = "src/plugin.ts"
version = "0.2.0-rc.1"           # 可选：与已安装版本不一致时构建失败
# provide = ["…"]                # 由 rutis 应用提供给插件的服务

# 组合挂载：彼此依赖的插件装进同一个 Cordis Context
[package.metadata.rutis-cordis.mounts.workspace]
group = [
    { name = "storage", plugin = "@deepseek-ai/dsh-storage" },
    { name = "storage_json", plugin = "@deepseek-ai/dsh-storage-json" },
    { name = "storage_domain", plugin = "@deepseek-ai/dsh-storage-domain" },
    { name = "sessions", plugin = "@deepseek-ai/dsh-session-persistence-jsonl" },
    { name = "workspace", plugin = "@deepseek-ai/dsh-workspace" },
]
```

`build.rs`：

```rust
fn main() {
    rutis_bridge::cordis::build::from_manifest().expect("generate Cordis bindings");
}
```

代码中引入全部挂载模块：

```rust
rutis_bridge::include_mounts!();   // 生成 credentials、workspace 等模块
```

生成的文件在 Cargo 构建目录里，不提交、不手工维护；插件或其类型变化时，Cargo 会自动重新生成。无法绑定的成员在构建时以 `warning` 列出位置和原因，其余成员照常生成。

### 3. 使用

```rust
let ctx = rutis::Ctx::root()?;
let view = ctx.plugin(credentials::Plugin::new(credentials::Config {
    dsh_home: Some("/tmp/home".into()),
    ..Default::default()
}));
(&view).await?;

// 服务是普通的 rutis 服务：依赖门控、换值、撤销都按 rutis 原生规则。
let store = ctx.require::<credentials::CredentialProvider>()?;
store.set(&credentials::CredentialRef::from("app/api-key"), "s3cret").await?;
```

| 能力 | 用法 |
| --- | --- |
| 方法 | 同步方法仍同步，返回 Promise 的方法是 `async fn`；返回 `Result<T, rutis_bridge::cordis::Error>`，Cordis 的业务错误为 `Error::Remote` |
| 数据类型 | 品牌类型为 newtype（`CredentialRef::from("…")`），接口为结构体，字面量联合为枚举；可选（`x?: T`）为 `Option`，`None` 发送 `undefined`；必填可空（`T \| null`）为 `Option`，`None` 发送 `null`；可选且可空为 `Option<Option<T>>` |
| 活对象 | 带方法的对象（例如 `Workspace`）是代理：属性 getter 实时读取，方法调用原对象，传回时还原为原对象；活对象的联合为 `ObjectRef`，与数据混合的联合为枚举 |
| 回调 | 函数参数传 Rust 闭包；返回的函数（例如注销函数）为 `RemoteFunction` |
| 取消 / 超时 | 丢弃返回的 future 即取消，Cordis 方法收到的 `AbortSignal` 会中止：`tokio::time::timeout(d, store.read_record(&key)).await` |
| 事件 | 不用声明：挂载里的 Cordis 监听器与 Rust 监听器在同一个逻辑总线上。Rust 一侧定义同名的事件类型（`impl Event`，`NAME` 为 Cordis 的事件名，派生 serde），或用 `EventKey::<rutis_bridge::events::Args>::dynamic(name)` 按名字收发；五种方式都可用（[在 Rust 应用里嵌入：事件](rust-host.md#事件)）。清单里的 `events` / `emits` 在 0.13 删除 |
| 宿主服务 | `provide` 中的服务生成 trait（例如 `SystemPromptHost`），实现后用生成的 `provide_system_prompt(&ctx, host)` 注册；挂载会等它就绪 |

### 部署

清单驱动的挂载按 npm 项目的相对位置定位运行时和插件。二进制换到别的机器或目录时，把 npm 项目（含已解析的 `node_modules`）一起部署，并用 `RUTIS_CORDIS_ROOT` 指向它：

```sh
cp -RL cordis /opt/app/cordis        # -L：file: 依赖等符号链接展开为实际文件
RUTIS_CORDIS_ROOT=/opt/app/cordis /opt/app/my-app
```

未设置时使用构建时的位置，开发期无需配置。用 `path` 挂载的 TypeScript 源文件应放在 npm 项目内，部署时随项目一起复制；运行时用默认的 `node_modules/@arcships/rutis-runtime`（不设 `runtime`）即可随项目移动。

### 4. Cordis 插件需要遵守的边界

跨进程后，少数由 JS 语言栈带来的行为无法保持，写成了 [需求 §5](../requirements-protocol-plugins.md) 的边界规则。事件不在其中：五种方式跨进程与同一进程里相同（见上面的[事件](#事件)），做不到的只有同步 `bail` 等待成环或等待断开的对端，这时明确报错。其余主要是：

- 直接 `ctx.set` 换值，rutis 侧在下一次调用该服务后才切换到新对象。
- 同步方法不能在执行中等待需要 Node 事件循环推进的结果，这类等待返回 `SyncWaitCycle`。
- 宿主提供的服务在 Cordis 侧是代理对象，`instanceof` 判断不成立。
- 插件不得长期阻塞事件循环；兼容层不为调用加超时，需要时用异步方法配合 `tokio::time::timeout`（超时即取消）。
- 插件里未捕获的异常或未处理的 Promise 拒绝会结束整个 Node 进程（Node 的默认规则），同一挂载里的插件一起停止。此后挂载的服务全部撤销，依赖它们的 rutis 插件停止等待；调用返回的 `Error::Transport` 说明进程如何结束。需要恢复时由应用卸载并重新挂载。

### 5. 常见构建错误

| 错误 | 处理 |
| --- | --- |
| `the Cordis plugins are not installed: run npm --prefix … ci` | 安装 npm 项目的依赖 |
| `… is not installed: add it to …/package.json` | 把插件加入 npm 项目并安装 |
| `… 1.0.0 is installed, 2.0.0 is required` | 让 npm 项目与 `version` 一致 |
| `… speaks protocol N, this rutis-bridge speaks M` | 安装与 crate 匹配的 `@arcships/rutis-runtime` |
| `native plugin dependencies are unresolved: … (name)`（运行时） | 缺少的服务需要放进同一个 `group`，或在 `provide` 中由 rutis 提供 |
