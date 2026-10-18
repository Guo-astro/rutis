# Cordis

rutis 和 Cordis（Node 插件框架）有两种互通方式：

- **Cordis 应用作为节点**：已有的 Cordis 应用接到 rutis 节点上，互相使用服务、转发事件，并让 rutis 在它里面运行插件。用 `@arcships/rutis-runtime/bridge`。
- **在 Rust 应用里挂载 Cordis 插件**：已发布的 Cordis 插件在真实的 Cordis 中运行，源码不改，Rust 侧拿到构建时从它的类型声明生成的 Rust 类型。用 `rutis-bridge` 的 `cordis` feature。

此外，Cordis 插件本身也可以作为 rutis-loader 的行运行，`rutis.json` 里写包名就行，包不用改，见 [写一个 TypeScript 插件](typescript-plugin.md#已有的-cordis-插件)：

- 同一个 Node 运行时里的 Cordis 行按 Cordis 的原生规则互相使用服务，拿到的是对象本身。
- 一行提供的服务自动投到 rutis，别的运行时的行、别的节点和 Rust 插件按名字使用；方法取服务对象上的函数（自身和原型链上的，不含 `constructor`），都按 `async` 调用。包的 `package.json` 里写了 `rutis.provides` 时按它（例如把某些方法标成 `sync`）。
- 同名服务已经有别的提供者（另一个运行时的行、Rust 插件、节点导入）时，后提供的那一行失败（装载后才提供的也一样），错误写明双方，例如 `service storage is already provided by row prompts: row storage cannot provide it too`。
- `rutis-host check` 不运行插件，看不到没有声明的服务：一行依赖的服务只可能由这类行提供时，写 `may be provided at runtime by …`，不算失败。

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
ctx.plugin(Events, { peer: 'main', out: ['tock'], in: ['tick'] })
```

- `Link` 维持与一个对端的连接，把它作为服务 `rutisPeer.<id>` 提供；其他功能依赖它，所以每次重连都会重新建立。
- `Export` 把 Cordis 服务按声明的方法形状给对端；`Import` 把对端的服务注册为 Cordis 服务（本地已有同名服务时拒绝）。
- `Host` 让对端在这里加载已安装的 npm 插件，并按行的 `isolate` / `inject` 运行；只对可信的对端打开。
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
    "@arcships/rutis-runtime": "0.11.0"
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
rutis-bridge = { version = "0.11", features = ["cordis"] }
tokio = { version = "1", features = ["full"] }

[build-dependencies]
rutis-bridge = { version = "0.11", features = ["cordis"] }

[package.metadata.rutis-cordis]
npm = "cordis"                   # 第 1 步的 npm 项目，相对 Cargo.toml
# runtime = "…"                  # 默认 <npm>/node_modules/@arcships/rutis-runtime

# 一个挂载 = 一个生成的模块
[package.metadata.rutis-cordis.mounts.credentials]
plugin = "@deepseek-ai/dsh-credentials-local"   # npm 包；或 path = "src/plugin.ts"
version = "0.2.0-rc.1"           # 可选：与已安装版本不一致时构建失败
events = ["credentials/record-updated"]         # 转发给 rutis 监听的 Cordis 事件
# emits = ["…"]                  # 由 rutis 发往 Cordis 监听的事件
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
| 事件 | `events` 中的事件生成 rutis 事件类型，用 `ctx.events().on(&ctx, &EventKey::<CredentialsRecordUpdated>::of(), listener)` 订阅；`emits` 中的事件由 rutis `emit` / `parallel` 发往 Cordis |
| 宿主服务 | `provide` 中的服务生成 trait（例如 `SystemPromptHost`），实现后用生成的 `provide_system_prompt(&ctx, host)` 注册；挂载会等它就绪 |

### 部署

清单驱动的挂载按 npm 项目的相对位置定位运行时和插件。二进制换到别的机器或目录时，把 npm 项目（含已解析的 `node_modules`）一起部署，并用 `RUTIS_CORDIS_ROOT` 指向它：

```sh
cp -RL cordis /opt/app/cordis        # -L：file: 依赖等符号链接展开为实际文件
RUTIS_CORDIS_ROOT=/opt/app/cordis /opt/app/my-app
```

未设置时使用构建时的位置，开发期无需配置。用 `path` 挂载的 TypeScript 源文件应放在 npm 项目内，部署时随项目一起复制；运行时用默认的 `node_modules/@arcships/rutis-runtime`（不设 `runtime`）即可随项目移动。

### 4. Cordis 插件需要遵守的边界

跨进程后，少数由 JS 语言栈带来的行为无法保持，写成了 [需求 §5](../requirements-protocol-plugins.md) 的边界规则，主要是：

- 跨边界的 `emit` 只是通知，不保证 `emit` 返回时 rutis 侧已处理；需要等待时用 `parallel`。
- 事件顺序只在同一侧内保证；waterfall / 有返回值的事件不转发。
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


## 把 Rust 服务导出给行

挂载的反方向：已有的 Rust 服务，由各语言的行（TypeScript、JavaScript、Python、Go）按名字使用；dispatcher 和 TypeScript 声明在构建时从 Rust 定义生成。`build.rs` 用 `rutis_bridge::cordis::build::Exports` 列出类型（构建依赖开 `cordis` feature）；应用用 `rutis_bridge::include_exports!()` 声明生成的模块，再用 `ServiceCatalog::export_all` 登记 `exports::all()`。用法见[在 Rust 应用里嵌入](rust-host.md#导出已有的-rust-服务)；完整示例：[`examples/rust-services`](../../examples/rust-services)。

| `Exports` | 作用 |
| --- | --- |
| `.service(name, path)` | 把 `path` 处的 struct 或 enum 导出为服务 `name`，在实例之外 |
| `.service_in(name, path, group)` | 导出在实例组 `group` 的每个实例里（同 `register_shared_instance`） |
| `.declared(path, typescript)` | 生成器推不出时，`path` 处 Rust 类型对应的 TypeScript 类型 |
| `.typescript(file)` | 声明再写一份到 `file`（相对 `Cargo.toml`），内容变了才写 |
| `.generate()` | 写出 `OUT_DIR/rutis_exports.rs` 和 `rutis_exports.d.ts`；`exports::TYPESCRIPT` 是声明的内容 |

### 源码从哪来

`path` 按应用自己的写法写：自己的类型写 `crate::…`，依赖的类型写依赖的 crate 名（按 `Cargo.toml` 里的名字，含改名）。构建脚本运行 `cargo metadata`（只取正在构建的目标平台，先离线），得到每个 crate 的入口文件和 Cargo 解析出的 feature，再用 syn 读模块树：`mod` 文件（含 `#[path]`）、`use`、glob 和 `pub use` 再导出（从私有模块再导出的类型也能找到）、类型别名（含泛型别名）。方法是该类型在其 crate 内任意位置的固有 `impl` 块里的公开 `&self` 方法。`#[cfg]` 会求值：`feature = "…"` 按 Cargo 解析出的 feature，目标平台的 `cfg` 按 `CARGO_CFG_*`，`test` 和 `doc` 视为关闭。只用稳定版 Rust：不为此编译或展开任何东西。

### 能传什么

| Rust | TypeScript | |
| --- | --- | --- |
| `bool`、`String`、`&str`、`char`、`PathBuf`、`&Path` | `boolean`、`string` | |
| `f32`、`f64`、`u8`–`u32`、`i8`–`i32` | `number` | |
| `u64`、`i64`、`usize`、`isize`、`u128`、`i128` | `number \| bigint` | 超出 ±(2^53−1) 时以 bigint 传（0.11 的值规则）；两种都接受 |
| `Option<T>` | `T \| null` | 末尾的 `Option` 参数可以不传；`Option` 字段是可选的 |
| `Vec<T>`、`&[T]`、集合、`VecDeque<T>`、`[T; N]` / 元组 | `T[]` / `[A, B]` | |
| `HashMap<K, V>`、`BTreeMap<K, V>` | `Record<string, V>` | |
| `Box<T>`、`Arc<T>`、`Rc<T>`、`Cow<T>` | `T` | |
| `serde_json::Value` | `unknown` | |
| `rutis_bridge::session::{Instant, Bytes, BigInt, ValueMap, ValueSet}` | `Date`、`Uint8Array`、`bigint`、`Map`、`Set` | 带标签的值 |
| derive 了 `Serialize` / `Deserialize` 的 struct 或 enum | `interface` 或联合类型，按 serde 写出的样子 | `rename`、`rename_all`、`rename_all_fields`、`tag`、`content`、`untagged`、`transparent`、`flatten`、`skip`、`default`、`skip_serializing_if`；泛型类型在 TypeScript 里也是泛型；文档注释变成 JSDoc |
| `impl Fn(A, ..) -> BoxFuture<'static, Result<X, E>>`、泛型 `F: Fn(..)`、`Arc<dyn Fn .. + Send + Sync>`、`Box<dyn Fn .. + Send>`（含别名） | `(arg0: A, ..) => X \| Promise<X>` | 按引用传的函数；最多四个参数，按值取 |
| `async fn`，或返回 `BoxFuture<'_, T>`、`Pin<Box<dyn Future<Output = T> ..>>`、`impl Future<Output = T>` | `Promise<T>` | 对调用方而言每个方法都返回 `Promise` |

参数用 serde 解码（任意 `Deserialize`），结果用 serde 编码（任意 `Serialize`）；Cargo 按真实签名检查每个生成的调用。

**按引用传的函数：** Rust 一侧拿到的函数会调用调用方的函数：它的 future 在函数返回、且返回的 Promise settle 之后才完成，所以订阅者处理完事件，原生一侧才继续；抛错或 reject 是这个 future 的 `Err`（要求 `E: From<rutis_bridge::session::Error>`，`CordisError`、`anyhow::Error`、装箱的错误都满足），走原生的失败路径。Rust 期望 `()` 时，函数返回的值被忽略。

**取消：** 调用方不再等待的调用（`AbortSignal`、丢弃 future）会在 Rust 一侧丢弃方法的 future，与所有调用相同。

**调用方行的上下文：** `&rutis::Ctx`（或 `Ctx`）参数在 TypeScript 里不是参数：它是发起调用的那一行的上下文（`rutis_bridge::session::caller_row()`），方法在上面登记的东西（`ctx.effect`、监听）属于那一行，随该行卸载或重启而移除，例如订阅。不在任何行里调用时（从 Rust、从对端节点），这样的方法失败，错误写明它需要调用方行的上下文。

### 错误

返回 `Result<T, E>` 的方法（`std::result::Result`、它的别名、`anyhow::Result`、`io::Result`）把 `Err(e)` 作为调用方语言里抛出的错误：JavaScript 的 `Error`、Python 的 `rutis.RemoteError`、Go 的 `*rutis.RemoteError`。它不是未分类的调用错误：

| `E` | `name` | `code` | `data` |
| --- | --- | --- | --- |
| 有 serde 的枚举 | 类型名 | serde 写出的变体：序列化成的字符串、外部标签变体的唯一键、内部或相邻标签变体的 `tag` 字段 | serde 值 |
| 没有 serde（或 `untagged`）的枚举 | 类型名 | 变体的 Rust 名（取自 derive 的 `Debug`） | 有 serde 时为 serde 值 |
| 有 serde 的结构体 | 类型名 | 序列化成的字符串，否则为类型名 | serde 值 |
| `String` | `Error` | 该字符串 | 该字符串 |
| `rutis::CordisError` | `CordisError` | 变体名 | — |
| `rutis_bridge::session::Error` | 原样抛出（转发的调用保留远端的错误） | | |
| 其他（`anyhow::Error`、`io::Error`） | 类型名 | 类型名 | — |

`message` 总是 `e` 的 `Display`。JavaScript 读 `error.code`、`error.data`；Python 读 `error.code`、`error.data`；Go 读 `remote.Code`、`remote.Data`（大整数为 `*big.Int`）；Rust 用 `Error::code()`、`Error::data()`。它们放在错误图里传，不认识它们的运行时仍拿到名字和消息。声明里每个错误类型有一个 `<Name>Thrown` 类型 `RustError<'Name', 'Code1' | 'Code2', Data>`，写在方法的 JSDoc 里（`@throws {CalendarErrorThrown}`）。

### 声明

`rutis_exports.d.ts` 包含每个服务类型的接口、它用到的数据类型、`RustError`、各 `<Name>Thrown` 类型，以及按名字列出服务的 `Services`：`ctx.use<Services['calendar']>('calendar')`。它能通过 `tsc --strict`（`examples/rust-services` 会检查）。`.typescript(…)` 写出的那份要提交：Rust 一侧变了，构建会改写它。

### 限制

- 宏写出的东西看不到：宏（derive 除外）写出的类型、`impl` 块或方法找不到。服务类型必须是具体的 struct 或 enum（不能是泛型，不能是 trait 对象）。
- 取 `&mut self`、返回借用、函数参数取引用（`Fn(&A)`）、除函数以外的泛型参数、除 `Fn` 以外的 `impl Trait`、不返回 `Result` 的 future 的函数参数，这样的方法会被跳过并给出警告。
- 数据类型需要生成器能读懂的 serde derive：手写的 `Serialize`、`#[serde(with / serialize_with / into / remote)]`，或者不读源码的 crate 的类型（`std`、`serde_json`、`chrono`、`uuid`、`url` 和带标签的值除外）需要 `.declared(path, typescript)`；在那之前用到它的方法被跳过，警告里写明类型。
- `#[cfg]` 按上面的规则求值；构建脚本看不到的 `cfg` 视为关闭。
- 方法的 future 必须是 `Send`（否则编译失败）。
