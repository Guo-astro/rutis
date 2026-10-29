# 做一个开放插件的宿主

[English](open-host.en.md)

读者：在自己的 Rust 产品里接入 rutis，并把插件能力开放给别的开发者。全篇的例子是命令行记事工具 `jot`：宿主提供 `notes` 服务，开放一个扩展点 `commands.register(name, handler)`，第三方用 TypeScript 或 Python 写命令。API 的完整参考见 [Rust 宿主](rust-host.md)。

## 1. 接入

`jot <命令> <参数>…` 运行一个命令。命令不在 jot 里：`commands/` 里的每个文件是一个插件，`.ts` 在 Node 运行时里运行，`.py` 在 Python 运行时里运行；插件启动时用 `commands.register` 登记自己的命令。

依赖：

<!-- example: jot file=jot/Cargo.toml -->
```toml
[package]
name = "jot"
version = "0.1.0"
edition = "2021"

[dependencies]
rutis = "0.19"
rutis-loader = { version = "0.19", features = ["node"] }
rutis-bridge = "0.19"
rutis-host = "0.19"
serde_json = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

宿主（`src/main.rs`）：

<!-- example: jot file=jot/src/main.rs -->
```rust,no_run
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rutis::host::*;
use rutis::Ctx;
use rutis_bridge::{session, LocalId, PeerId};
use rutis_host::connect::SpawnPlugin;
use rutis_host::languages;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, Layer, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
use serde_json::{json, Value};

/// The notes: one per line of `notes.txt`.
pub struct Notes {
    file: PathBuf,
}

#[rutis::service]                                     // 插件按名字 "notes" 使用它
impl Notes {
    /// Add a note.
    pub fn add(&self, text: String) -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.file)?;
        writeln!(file, "{text}")
    }

    /// Every note, oldest first.
    pub fn list(&self) -> Vec<String> {
        let text = std::fs::read_to_string(&self.file).unwrap_or_default();
        text.lines().map(str::to_owned).collect()
    }
}

/// A command: given its arguments, the text jot prints.
pub type Run = Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, rutis::Result<String>> + Send + Sync>;

/// The commands plugins add.
#[derive(Clone, Default)]
pub struct Commands(Arc<Mutex<BTreeMap<String, Run>>>);

#[rutis::service]                                     // 插件按名字 "commands" 使用它
impl Commands {
    /// Add the command `name`. It goes when the plugin that added it unloads.
    pub fn register(&self, ctx: &Ctx, name: String, run: Run) -> rutis::Result<()> {
        self.0.lock().unwrap().insert(name.clone(), run);
        let commands = self.0.clone();
        ctx.effect(|| async move {                    // ctx 是调用方那一行的：它卸载时运行
            commands.lock().unwrap().remove(&name);
            Ok(())
        })?;
        Ok(())
    }
}

impl Commands {                                       // 没有标注：只给 jot 自己用
    fn get(&self, name: &str) -> Option<Run> {
        self.0.lock().unwrap().get(name).cloned()
    }

    fn names(&self) -> Vec<String> {
        self.0.lock().unwrap().keys().cloned().collect()
    }
}

/// commands/ 里每个文件一行：.ts 在 Node 里运行，.py 在 Python 里运行。
fn command_rows(dir: &Path) -> std::io::Result<Vec<Value>> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else { continue };
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("ts") => rows.push(json!({ "id": id, "name": path })),
            Some("py") => rows.push(json!({ "id": id, "name": format!("py:{id}") })),
            _ => {}
        }
    }
    Ok(rows)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((name, args)) = args.split_first() else {
        eprintln!("usage: jot <command> [<argument>...]");
        std::process::exit(2);
    };
    let dir = std::env::current_dir()?;               // 绝对路径：每个运行时的进程在它自己的目录里启动

    let root = Ctx::root()?;
    root.provide(LocalId(PeerId::new("main")?))?;    // jot 与运行时的会话里，jot 问候用的 id
    root.provide(Notes { file: dir.join("notes.txt") })?;
    let commands = Commands::default();
    root.provide(commands.clone())?;

    let catalog = ServiceCatalog::new();
    let node = SpawnPlugin::new("node", languages::node::launcher(&dir.join("node_modules/@arcships/rutis-runtime")), dir.join("package.json"));
    let python = SpawnPlugin::new("py", languages::python::launcher(&dir.join("commands")), dir.join("commands"))
        .interpreter(dir.join(".venv/bin/python"));
    let sessions = Arc::new(SessionResolver::npm(&catalog, "node")); // py:<模块> 在 py 上；文件、npm 名在 node 上
    sessions.runtime(node.handle());
    sessions.runtime(python.handle());
    let runtimes = [node.handle(), python.handle()];
    root.plugin(node);
    root.plugin(python);

    let loader = LoaderPlugin::new(
        Chain::new().with_shared(sessions.clone()),
        LoaderOptions { catalog, ..LoaderOptions::default() },
    );
    let handle = loader.handle();
    sessions.attach(&handle);
    root.plugin(loader).await?;
    sessions.follow_row("node");
    sessions.follow_row("py");

    for runtime in &runtimes {
        runtime.ready().await;
    }
    let rows: Vec<Patch> = serde_json::from_value(json!([{ "insert": command_rows(&dir.join("commands"))? }]))?;
    handle.reconcile(vec![Layer::new("commands", rows)], None).await?; // 每一行的 apply 都结束后返回

    let code = match commands.get(name) {
        Some(run) => match run(args.to_vec()).await {
            Ok(text) => {
                println!("{text}");
                0
            }
            Err(error) => {
                // 命令自己的用法错误（code 是 "usage"）照原样说给用户
                match rutis_bridge::thrown(&error).filter(|thrown| thrown.code() == Some("usage")) {
                    Some(session::Error::Remote { message, .. }) => eprintln!("jot: {message}"),
                    _ => eprintln!("jot: {name} failed: {error}"),
                }
                1
            }
        },
        None => {
            eprintln!("jot: no command {name} (commands: {})", commands.names().join(", "));
            2
        }
    };
    root.shutdown().await?;
    std::process::exit(code);
}
```

两个第三方写的命令。插件只依赖 `@arcships/rutis` 或 `rutis`，不知道宿主是 jot：

<!-- example: jot file=jot/commands/count.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

interface Notes {
  list(): Promise<string[]>
}

interface Commands {
  register(name: string, run: (args: string[]) => Promise<string>): Promise<void>
}

export default definePlugin({
  inject: ['notes', 'commands'],
  async apply(ctx) {
    const notes = ctx.use<Notes>('notes')
    await ctx.use<Commands>('commands').register('count', async () => {
      const all = await notes.list()
      const words = all.join(' ').split(/\s+/).filter(Boolean).length
      return `${all.length} notes, ${words} words`
    })
  },
})
```

<!-- example: jot file=jot/commands/shout.py -->
```python
from rutis import define_plugin


class UsageError(Exception):
    code = "usage"


async def apply(ctx, config):
    notes = ctx.use("notes")

    async def shout(args):
        if not args:
            raise UsageError("say what to shout: jot shout <text>")
        text = " ".join(args).upper()
        await notes.add(text)
        return f"added: {text}"

    await ctx.use("commands").register("shout", shout)


plugin = define_plugin(apply, inject=["notes", "commands"])
```

装上两个运行时（Node 22 或更高，Python 3.12 或更高），运行：

<!-- example: jot run -->
```bash
cd jot
npm init -y
npm install @arcships/rutis-runtime @arcships/rutis
python3 -m venv .venv
.venv/bin/pip install rutis
cargo run -q -- shout buy milk
cargo run -q -- count
```

```text
added: BUY MILK
1 notes, 2 words
```

### 每个零件做什么

| 零件 | 做什么 | 为什么这样组合 |
| --- | --- | --- |
| `Ctx::root()` | 插件树的根，用当前的 tokio 运行时 | 宿主自己的服务（`notes`、`commands`）提供在根上，所有行都看得到 |
| `root.provide(…)` | 提供服务；类型标了 `#[rutis::service]` 时同时按名字对外（第 2 节） | 不用另写登记表 |
| `LocalId` | jot 与运行时问候时用的 id | 没有它会话失败：`the host provides no id for its sessions` |
| `SpawnPlugin::new(名字, launcher, anchor)` | 本机的语言运行时，一个运行时一个进程（即 rutis-host 的 `rutis-host/spawn`）：`rutis_host::languages` 的 `node::launcher` / `python::launcher` 说明进程怎样启动，`anchor` 是插件从哪里解析；`root.plugin(runtime)` 启动进程、在它的通道上开一个会话，卸载时结束它 | 它是插件，随根关闭；它的行等它 |
| `SessionResolver` | 把行名解析成会话对面的插件，本机运行时与远程的都一样：`<运行时名>:<插件>`（这里是 `py:shout`）；用 `npm(&catalog, "node")` 创建时还认 npm 名和文件，放在运行时 `node` 上；`runtime(handle)` 让 `Loader::status()` 列出这个运行时 | 所有运行时共用一个解析器，放在 `Chain` 最后 |
| `ServiceCatalog` | 服务名到键的表，loader 与各解析器共用一份 | 行按名字等的服务和宿主按类型提供的服务对得上 |
| `Chain` | 按顺序问它的解析器，第一个认得行名的解析它 | 几种语言的行写在同一份配置里 |
| `with_shared` | 把一个也被别处持有的解析器放进 `Chain` | 同一个 `SessionResolver` jot 还要用来 `attach`、`follow_row` |
| `LoaderPlugin` / `handle()` | 按行装插件；`handle()` 是之后操作它的 `Loader` | 先拿 `handle`，再把插件交给根 |
| `attach` / `follow_row` | `attach(&handle)` 告诉解析器它为哪个 loader 解析行；`follow_row("py")` 让这个运行时一运行起来就放行它的行。运行时重启后，它的行自己重新解析，用上插件现在的声明 | `attach` 在挂载 loader 之前，`follow_row` 在之后 |
| `Layer`、`Patch` | 行的来源：几层，后面的层覆盖前面的；一层是若干 `Patch`（`insert` 加行，或按 `id` 改字段） | 宿主自带的行、用户加的行各放一层（第 4 节） |
| `root.shutdown()` | 卸载全部插件，等每个清理结束，运行时进程随之结束 | 不限时：要限时用 `shutdown_with_timeout`（[默认输出](rust-host.md#默认输出)） |

在另一台机器上运行插件见 [rutis-host.md 的远程一节](rutis-host.md#远程运行时与设备)。

### 等就绪

`reconcile` 在每一行都装载过一遍后才返回：行的 `apply` 结束（这里命令已经登记好），或者失败（列在报告的 `failures` 里，默认的错误汇集也打印一行 `<行>: failed: …`）。运行时还在启动时，它的行先等着它，报告里没有它们的结果，所以先等 `RuntimeHandle::ready()`。等一个没有谁提供的服务的行不卡住 `reconcile`：它继续等，10 秒后错误汇集报一次在等什么。

一行的 `apply` 一直不返回（在等一个不返回的调用）时，`reconcile` 也不返回。rutis 不给 `apply` 限时；等多久由宿主决定。想知道卡在哪里，等的同时打印状态：

```rust,no_run
# use rutis_loader::{Layer, Loader};
# async fn demo(handle: Loader, layers: Vec<Layer>) -> Result<(), Box<dyn std::error::Error>> {
use std::time::Duration;

let reconcile = handle.reconcile(layers, None);
tokio::pin!(reconcile);
tokio::select! {
    report = &mut reconcile => { report?; }
    _ = tokio::time::sleep(Duration::from_secs(3)) => {
        eprintln!("jot: still starting after 3s\n{}", handle.status());
        reconcile.await?;                             // 接着等；或者在这里结束
    }
}
# Ok(())
# }
```

`commands/stuck.ts` 的 `apply` 里写 `await new Promise(() => {})` 时：

```text
jot: still starting after 3s
rows:
  count: running
  stuck: starting
  shout: running
services:
  commands: host
  loader: host
  notes: host
runtimes:
  node: running (pid 57684)
  py: running (pid 57685)
calls:
  none
```

`starting` 的那一行就是没返回的 `apply`；它在调用一行的服务时，`calls` 里写出是哪个调用。

### 路径与环境

- 每个运行时的进程在它自己的目录里启动（Python：项目目录，这里是 `commands/`；Node：运行时的包目录），收到的路径照原样使用：写绝对路径，相对路径会从那个目录算起。
- `languages::python::launcher(目录)` 从这个目录导入插件模块，用 `SpawnPlugin::interpreter(…)` 选解释器（环境里要装着 `rutis`）；没装的模块或 rutis 的源码目录用 `.python_path(目录)` 放到 `PYTHONPATH` 前面。
- `SpawnPlugin::env(名字, 值)` 给运行时进程加环境变量；要用别的方式启动进程，自己写一个 `rutis_bridge::runtime::Launcher`（`new`、`arg`、`env`、`cwd`），交给 `SpawnPlugin::new(名字, launcher, anchor)`。进程最后的参数是 `--id <名字>` 和 `anchor`。
- 行名不是文件、npm 名或 `<运行时名>:<模块>` 时（比如 jot 想让用户写 `wordcount`），实现 `rutis_loader::Resolver`：认得的名字返回解析结果，不认得的返回 `LoaderError::NotFound`，`Chain` 接着问下一个；用到的类型在 `rutis_loader::resolve` 里。编进宿主的 Rust 插件用 `Builtins`（[装 Rust 插件](rust-host.md#装-rust-插件)）。

## 2. 定义插件面

插件面是宿主对外的服务：jot 的是 `notes` 和 `commands`。

### 用 `#[rutis::service]` 写服务

`impl` 块上加 `#[rutis::service]`，照常 `provide`，每种语言的行就能按名字使用它（上面的 `Notes`、`Commands`）：

- 对外的是这个块里公开的 `&self` 方法，名字按 camelCase；对调用方都是异步的（TypeScript 里返回 Promise，Python 里可 await）。
- 服务名默认是类型名、第一个词小写（`Notes` 是 `notes`），`#[service(name = "…")]` 改名。
- `Err(e)` 在调用方那里是抛出的错误，带 `name`、`code`、`message`、`data`。
- 没标注的方法和类型不对外：jot 自己用的 `Commands::get` 写在另一个没标注的 `impl` 里。

能传什么、错误怎样抛的完整规则见 [在类型上标注 `#[rutis::service]`](rust-host.md#在类型上标注-rutisservice) 与 [导出服务：参考](rust-host.md#导出服务参考)。

### 回调注册

`commands.register(name, run)` 的 `run` 是插件里的函数，按引用传过来：Rust 拿到的 `Run` 调用时调用的是插件进程里的那个函数，它的 Promise（或 Python 的协程）结束后 future 才完成；插件抛错时 future 是 `Err`。

- 参数类型写函数类型（`impl Fn`、`Arc<dyn Fn … + Send + Sync>`，可以用别名），返回 `BoxFuture<'static, Result<…>>`，最多四个参数。
- `&Ctx` 参数不是插件传的：它是发起调用的那一行的上下文。在它上面登记的清理（`ctx.effect`）在这一行卸载或重启时运行，所以插件卸载时它的命令自动去掉，插件不用写注销。
- 不在任何行里调用时（从 Rust、从对端节点），带 `&Ctx` 的方法失败，错误写明它需要调用方行的上下文。

### 是哪一行在调用

`rutis_bridge::session::caller_row()` 给出调用方的行：`id()` 是 loader 的行 id。比如两个插件登记同一个命令名时，拒绝后一个并说出是哪一行：

```rust,no_run
# use std::collections::BTreeMap;
# use std::sync::{Arc, Mutex};
# use rutis::host::BoxFuture;
# use rutis::Ctx;
# pub type Run = Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, rutis::Result<String>> + Send + Sync>;
use rutis_bridge::session::caller_row;

/// The commands, each with the row that added it.
#[derive(Clone, Default)]
pub struct Commands(Arc<Mutex<BTreeMap<String, (String, Run)>>>);

#[rutis::service]
impl Commands {
    /// Add the command `name`, unless another plugin has.
    pub fn register(&self, ctx: &Ctx, name: String, run: Run) -> Result<(), String> {
        let row = caller_row().map(|row| row.id().to_owned()).unwrap_or_default();
        let mut commands = self.0.lock().unwrap();
        if let Some((owner, _)) = commands.get(&name).filter(|(owner, _)| *owner != row) {
            return Err(format!("{name} is already added by {owner}"));
        }
        commands.insert(name.clone(), (row, run));
        let all = self.0.clone();
        ctx.effect(|| async move {
            all.lock().unwrap().remove(&name);
            Ok(())
        })
        .map_err(|error| error.to_string())?;
        Ok(())
    }
}
```

在方法体里、第一个 `.await` 之前读它（上面的方法是同步的）。来自 Rust 或对端节点的调用是 `None`。其余见 [调用方的行](rust-host.md#调用方的行)。

### 给插件作者的类型

`rutis-host types` 从源码读出标了 `#[rutis::service]` 的类型，写出 TypeScript 声明和 Python 类型（[声明](rust-host.md#声明)）：

```sh
rutis-host types . -o jot.d.ts --python jot_types.py
```

写出的 `jot.d.ts` 里，`Commands` 是：

```ts
/** The commands plugins add. */
export interface Commands {
  /**
   * Add the command `name`. It goes when the plugin that added it unloads.
   * @throws {ErrorThrown}
   */
  register(name: string, run: (arg0: string[]) => string | Promise<string>): Promise<void>
}
```

插件作者写 `ctx.use<Services['commands']>('commands')`（Python：`commands: Commands = ctx.use("commands")`），编辑器和 `tsc`、pyright 在运行前检查调用；不用它们也照样运行。两个文件交给插件作者（第 5 节放进模板里），Rust 一侧改了就重新生成。

## 3. 管住插件

第三方插件不归你管：它可能崩溃、卡死。rutis 给你的是把插件放进各自的进程，和本机运行时的问候时限；调用限时、能用哪些服务由你自己决定（见本节末尾）。

### 每个插件一个进程

同一个运行时里的插件共用一个进程：一个插件崩溃或卡死，其他插件跟着停下。给每个第三方插件一个自己的运行时：一个用它自己名字的 `SpawnPlugin`；它的行名写 `<运行时名>:<npm 名或文件>`，由 `SessionResolver` 解析：

```rust,no_run
# use std::sync::Arc;
# use rutis::host::*;
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let root = rutis::Ctx::root()?;
# root.provide(rutis_bridge::LocalId(rutis_bridge::PeerId::new("main")?))?;
# let app = std::env::current_dir()?.join("app");
# struct Command { id: String, dir: std::path::PathBuf }
# let commands: Vec<Command> = Vec::new();
use rutis_host::connect::SpawnPlugin;
use rutis_host::languages;
use rutis_loader::runtime::SessionResolver;
use rutis_loader::{Chain, LoaderOptions, LoaderPlugin, ServiceCatalog};

let catalog = ServiceCatalog::new();
let runtime_package = app.join("node_modules/@arcships/rutis-runtime");
let sessions = Arc::new(SessionResolver::new(&catalog));  // 行 <运行时名>:<插件>，所有运行时的
let mut fibers = Vec::new();
for command in &commands {                                // 每个第三方命令一个进程
    let runtime = SpawnPlugin::new(
        &command.id,                                      // 运行时名：两个以上的 a-z、0-9、-
        languages::node::launcher(&runtime_package),
        command.dir.join("package.json"),
    );
    sessions.runtime(runtime.handle());
    fibers.push(root.plugin(runtime));
}
let loader = LoaderPlugin::new(Chain::new().with_shared(sessions.clone()), LoaderOptions { catalog, ..LoaderOptions::default() });
sessions.attach(&loader.handle());
root.plugin(loader).await?;
for command in &commands {
    sessions.follow_row(&command.id);
}
// 行：{ "id": "wordcount", "name": "wordcount:jot-wordcount" }
# Ok(())
# }
```

- 几个插件共用一个进程：它们的行写同一个前缀。
- 用 `SessionResolver::npm(&catalog, "node")` 创建时，不带前缀的行也放在默认的 Node 运行时 `node` 上；带别的运行时前缀的行仍归那个运行时。
- 每次解析都问运行时，不按包的 `version` 缓存。

### 问候时限

本机运行时的进程启动后 30 秒内没有问候就被结束：运行时停下（`the runtime did not greet within 30s`），它的行等它。调试器停在第一行（`--inspect-brk`）时，用 `SpawnPlugin::greeting(Duration::MAX)` 不限时：

```rust,no_run
# use std::time::Duration;
# use rutis_host::connect::SpawnPlugin;
# use rutis_host::languages;
# let app = std::path::PathBuf::from("/srv/app");
let runtime = SpawnPlugin::new("wordcount", languages::node::launcher(&app.join("node_modules/@arcships/rutis-runtime")), app.join("package.json"))
    .env("NODE_OPTIONS", "--inspect-brk")
    .greeting(Duration::MAX);
```

不限时，一个启动了却一直不问候的进程会一直停在启动中；`Loader::status()` 里看得到：

```text
rows:
  wordcount: waiting for runtime wordcount (starting)
runtimes:
  wordcount: starting (pid 4242)
```

Windows 上进程连上宿主之后才有进程号，这一行只写 `wordcount: starting`。

### 不是限制的东西

- rutis 不给调用限时：调用方自己限时（Rust 的 `tokio::time::timeout`、Python 的 `asyncio.wait_for` 等）。插件卡在同步代码里、取消停不下时，用 `SpawnPlugin::processes()` 的 `kill()` 结束它的进程，再对 `root.plugin(runtime)` 返回的 fiber 调用 `restart()`：新的进程，它的行重新装载。
- rutis 不管插件能用哪些服务：哪些服务给哪个插件是你的业务，由你自己分配。
- 服务按名字共享，这不是权限。
- 同一进程里的插件能用语言本身的办法互相拿到对方（全局变量、模块缓存）；要隔开，放进不同的运行时。
- 插件进程有宿主用户的全部权限（文件、网络、子进程）。rutis 不做沙箱，也不限内存、CPU。

各选项的参考见 [Rust 宿主：本机运行时的选项](rust-host.md#本机运行时的选项)。

## 4. 管理插件

### 装、升级、卸载

嵌入 rutis 的宿主自己用包管理器装插件的包，装到运行时找得到的地方，再加一行：

| 插件 | 装到哪里（jot） | 行的 `name` |
| --- | --- | --- |
| npm 包 | `npm install <包>`，装进 jot 的 `node_modules`（`node` 运行时的 `SpawnPlugin` 作为 `anchor` 的 `package.json` 所在目录） | 包名 |
| PyPI 包 | `.venv/bin/pip install <包>`，装进 `.interpreter(…)` 的环境 | `py:<入口点或模块>` |
| 单个文件 | 不用装 | 绝对路径（Node），`py:<模块>`（Python 项目目录里） |

升级是装新版本，再重新加载这一行（`Loader::reload`，见第 5 节）或重启它的运行时（Node 已经导入的模块不会重新导入）；卸载是删掉这一行，再卸掉包。rutis-host 的 `add`、`remove`、`upgrade` 对 `rutis.json` 做的就是这些，每种语言装到哪里、行名怎样写见 [rutis-host：宿主怎样装插件](rutis-host.md#宿主怎样装插件)。

### 增删行、改配置

用户加的命令放在一层可编辑的层里（这里叫 `user`）：`Loader` 的修改方法只改这一层，改完交给 `LoaderOptions::persist` 保存。jot 把它存在 `jot.json`：

```rust,no_run
# use std::path::PathBuf;
# use std::sync::Arc;
# use rutis_loader::{Chain, Layer, Loader, LoaderOptions, LoaderPlugin, Patch, ServiceCatalog};
use rutis::host::BoxFuture;
use rutis_loader::edit::{Edit, Editable, NewEntry, Persist, PersistError, Version};
use rutis_loader::LoaderError;
use serde_json::json;

/// The rows the user added, kept in `jot.json`.
struct JotFile(PathBuf);

impl JotFile {
    fn read(&self) -> String {
        std::fs::read_to_string(&self.0).unwrap_or_else(|_| "[]".into())
    }
}

impl Persist for JotFile {
    fn load<'a>(&'a self, _layer: &'a str) -> BoxFuture<'a, Result<(Vec<Patch>, Version), LoaderError>> {
        Box::pin(async move {
            let text = self.read();
            let patches = serde_json::from_str(&text)
                .map_err(|error| LoaderError::InvalidEntry(format!("jot.json: {error}")))?;
            Ok((patches, Version(text)))
        })
    }

    fn save<'a>(
        &'a self,
        _layer: &'a str,
        expected: &'a Version,
        _edits: &'a [Edit],
        patches: &'a [Patch],
    ) -> BoxFuture<'a, Result<Version, PersistError>> {
        Box::pin(async move {
            if self.read() != expected.0 {
                return Err(PersistError::Conflict);  // 别人改过：loader 重新读，再做一遍这次修改
            }
            let text = serde_json::to_string_pretty(patches).map_err(|e| PersistError::Failed(e.to_string()))?;
            std::fs::write(&self.0, &text).map_err(|e| PersistError::Failed(e.to_string()))?;
            Ok(Version(text))
        })
    }
}

# async fn demo(dir: PathBuf, chain: Chain, catalog: ServiceCatalog, rows: Vec<Patch>, root: rutis::Ctx) -> Result<(), Box<dyn std::error::Error>> {
# use rutis::host::CtxHost;
let file = JotFile(dir.join("jot.json"));
let (user, version) = file.load("user").await?;
let loader = LoaderPlugin::new(chain, LoaderOptions { catalog, persist: Arc::new(file), ..LoaderOptions::default() });
let handle = loader.handle();
root.plugin(loader).await?;
handle
    .reconcile(
        vec![Layer::new("commands", rows), Layer::new("user", user)],
        Some(Editable::new("user", version)),
    )
    .await?;

// jot add-command ./today.ts
let file = std::fs::canonicalize("today.ts")?;
let entry = NewEntry { id: Some("today".into()), name: file.display().to_string(), ..NewEntry::default() };
handle.create(entry, None, None).await?;               // 装上并启动；起不来时什么都不加
handle.update("today", json!({ "format": "iso" })).await?;  // 换配置：这一行重启
handle.set_disabled("today", true).await?;             // 停用，行还在
handle.remove("today").await?;                         // 卸载并删掉
# Ok(())
# }
```

- 每个修改方法装载完受影响的行才返回；新行起不来（找不到插件、配置不符合 Schema、`apply` 失败）时修改撤回，返回的错误写明原因，例如 `a row today already exists`。
- 只有可编辑层里的行能改：`commands` 层里的行（jot 自带的命令）改不了，错误写明这一行不属于可编辑层。
- 没有可编辑层时（`reconcile(…, None)`，第 1 节的 jot），`create` 与 `remove` 失败：`this host keeps no editable configuration, so rows cannot be added or removed`。
- 行的完整格式（分组、`isolate`、`inject`）与修改的种类（`Edit`）见 [rutis-loader 设计](../design-rutis-loader-2026-10-02.md)。

### 看一行现在怎样

`handle.get(id)` 给出一行的配置与状态（`EntryStatus`），`handle.status()` 给出全部行、服务、运行时与进行中的调用，见第 6 节。

### 插件管理插件

用 `LoaderPlugin` 的宿主还按名字提供服务 `loader`，与 Cordis 的 `ctx.loader` 同名同形：`create(row)`、`reload(id)`、`remove(id)`、`status()`。插件 `inject: ['loader']` 就能用，比如一个列出插件的命令：

<!-- example: jot file=jot/commands/plugins.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

interface Loader {
  status(): Promise<{ rows: { id: string; state: string; reason?: string }[] }>
}

interface Commands {
  register(name: string, run: (args: string[]) => Promise<string>): Promise<void>
}

export default definePlugin({
  inject: ['loader', 'commands'],
  async apply(ctx) {
    const loader = ctx.use<Loader>('loader')
    await ctx.use<Commands>('commands').register('plugins', async () => {
      const { rows } = await loader.status()   // 在命令里调用，不在 apply 里
      return rows.map(row => `${row.id}: ${row.state}`).join('\n')
    })
  },
})
```

<!-- example: jot run -->
```bash
cargo run -q -- plugins
```

```text
count: running
plugins: running
shout: running
```

- `loader` 的方法在 `apply` 返回之后调用（服务方法、事件里）：在一行自己的 `apply` 里调用会等到这个 `apply` 结束，所以会卡住。
- `create` 与 `remove` 改的是可编辑层，和上面的 Rust 方法一样；没有可编辑层时失败。
- 任何插件都能 `inject` 它，rutis 不加权限：不想让某个插件管理插件，就不装那个插件。
- 各方法的参数、返回值与出错文字见 [插件 API：用 `loader` 管理插件](plugin-api.md#用-loader-管理插件)。

## 5. 服务好插件作者

### 模板与假服务

插件作者没有 jot 也要能写、能测、能试。交给他们一个模板：`rutis-host new` 生成的项目，加上宿主给的三样东西——第 2 节生成的类型、假的 `notes` 与 `commands`、把它们放进 `rutis.dev.json` 的那几行。插件作者照着做：

<!-- example: jot-command run -->
```bash
npx @arcships/rutis-host new count --lang node
cd count
npm install
```

宿主生成的类型（`rutis-host types` 写出，原样放进模板）：

<!-- example: jot-command file=count/src/jot.d.ts -->
```ts
// Generated from the service declarations; do not edit.

/** What a method's `Err(e)` throws: named after `e`'s type, `code` its variant, `message` its `Display`, `data` its serde value. */
export interface RustError<Name extends string = string, Code extends string = string, Data = unknown> extends Error {
  readonly name: Name
  readonly code: Code
  readonly data: Data
}

/** The notes: one per line of `notes.txt`. */
export interface Notes {
  /**
   * Add a note.
   * @throws {IoErrorThrown}
   */
  add(text: string): Promise<void>
  /** Every note, oldest first. */
  list(): Promise<string[]>
}

/** The commands plugins add. */
export interface Commands {
  /**
   * Add the command `name`. It goes when the plugin that added it unloads.
   * @throws {ErrorThrown}
   */
  register(name: string, run: (arg0: string[]) => string | Promise<string>): Promise<void>
}

/** What a method returning `Err(Error)` throws. */
export type ErrorThrown = RustError<"Error", string, undefined>

/** What a method returning `Err(io::Error)` throws. */
export type IoErrorThrown = RustError<"Error", "Error", undefined>

/** The exported services by name: `ctx.use<Services['name']>('name')`. */
export interface Services {
  notes: Notes
  commands: Commands
}
```

假的 jot：`notes` 里有两条笔记，`commands` 多一个 `run`，用来试登记了的命令：

<!-- example: jot-command file=count/dev/fake-jot.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

const commands = new Map<string, (args: string[]) => Promise<string>>()

export default definePlugin({
  provides: { notes: { add: 'async', list: 'async' }, commands: { register: 'async', run: 'async' } },
  apply(ctx) {
    const notes = ['buy milk', 'call Ada back']
    ctx.provide('notes', {
      add: async (text: string) => { notes.push(text) },
      list: async () => notes,
    })
    ctx.provide('commands', {
      register: async (name: string, run: (args: string[]) => Promise<string>) => { commands.set(name, run) },
      run: async (name: string, args: string[] = []) => commands.get(name)!(args),
    })
  },
})
```

<!-- example: jot-command file=count/rutis.dev.json -->
```json
{
  "format": 1,
  "rows": [
    { "id": "count" },
    { "id": "jot", "name": "./dev/fake-jot.ts" }
  ]
}
```

插件作者写的命令和它的测试（两个文件一起换掉模板里的）：

<!-- example: jot-command file=count/src/index.ts -->
```ts
import { definePlugin } from '@arcships/rutis'
import type { Services } from './jot.js'

export default definePlugin({
  inject: ['notes', 'commands'],
  async apply(ctx) {
    const notes = ctx.use<Services['notes']>('notes')
    await ctx.use<Services['commands']>('commands').register('count', async () => {
      const all = await notes.list()
      const words = all.join(' ').split(/\s+/).filter(Boolean).length
      return `${all.length} notes, ${words} words`
    })
  },
})
```

<!-- example: jot-command file=count/test/index.test.ts -->
```ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

test('counts the notes and their words', async () => {
  const commands = new Map<string, (args: string[]) => Promise<string>>()
  const t = await load(plugin, {
    services: {
      notes: { list: async () => ['buy milk', 'call Ada back'] },
      commands: { register: async (name: string, run: (args: string[]) => Promise<string>) => { commands.set(name, run) } },
    },
  })
  assert.equal(await commands.get('count')!([]), '2 notes, 5 words')
  await t.unload()
})
```

<!-- example: jot-command run -->
```bash
npm test
```

<!-- example: jot-command run until="count: running" -->
```bash
npx rutis-host dev
```

`dev` 运行时，另开一个终端试这个命令：

```console
$ npx rutis-host call commands.run '"count"'
"2 notes, 5 words"
```

- 测试里的假服务直接写在 `load` 的 `services` 里；`dev` 里的假服务是 `rutis.dev.json` 里的一行。
- 用 Python 写命令的作者用 `--python` 写出的 `jot_types.py`；假的 jot 可以照样用 TypeScript 写：`rutis.dev.json` 里不同语言的行可以放在一起。
- 测试工具、`dev`、`rutis-host call` 的用法见各语言的插件指南（[TypeScript](typescript-plugin.md#6-测试)、[Python](python-plugin.md#6-测试)）与 [rutis-host：用 dev 开发插件](rutis-host.md#用-dev-开发插件)。

### 开发循环

宿主一侧要在不重启的情况下换上插件的新代码时（jot 想做一个 `--watch`），重新加载那一行即可：每次解析都重新问运行时，没有要先忘掉的：

```rust,no_run
# use rutis_loader::Loader;
# async fn demo(handle: Loader) {
use rutis::host::FiberState;
use rutis_loader::report::{cannot_reload, EntryStatus};

let id = "count";                                     // 文件变了的那一行
match handle.reload(id).await {
    Ok(_) => eprintln!("{id}: reloaded"),
    Err(error) => {
        let running = handle
            .get(id)
            .is_some_and(|row| matches!(&row.status, EntryStatus::Running(s) if s.state == FiberState::Active));
        eprintln!("{}", cannot_reload(id, &error, running)); // 与 rutis-host dev 打印的同一句
    }
}
# }
```

- 新代码导入不了（语法错误）时，这一行继续运行原来的版本：`count: cannot reload: …; the previous version keeps running`。新代码能导入、`apply` 失败时，这一行以那个错误失败，改好之前不运行。
- 只重新导入这一行自己的入口模块：它导入的其他模块变了，重启它的运行时（对运行时的 fiber 调用 `restart()`）。
- 用到这一行所提供服务的行随它重启。

### 出错信息改成宿主的话

插件的错误到宿主时带着 `name`、`code`、`message`、`data`、`stack` 和出错的行（`site`）。`rutis_bridge::thrown(&error)` 取出它们，宿主按 `code` 决定怎样说，不比较文字。第 1 节的 jot 把 `code` 为 `usage` 的错误照原样说给用户，其余的写明是哪个命令：

```console
$ cargo run -q -- shout
jot: say what to shout: jot shout <text>
```

不这样处理时打印的是 `jot: shout failed: UsageError: say what to shout: jot shout <text>`。

- `code` 由插件给：TypeScript 与 Python 是错误的字符串属性 `code`，Go 是方法 `Code()`（[插件 API](plugin-api.md)）。在给插件作者的文档里写明宿主认哪些 `code`。
- rutis 自己的拒绝也有 `code`（`rutis_bridge::session::code`：`not-declared`、`unavailable`、`no-method`、`bad-argument`、`bad-value`、`peer-gone`、`bad-config`）。
- 行启动失败、运行时退出这类事由错误汇集报告，默认写到标准错误（`count: failed: …`）。要改成宿主的话，换掉汇集：`Ctx::root_with_sink(handle, sink)`，见 [默认输出](rust-host.md#默认输出)。

## 6. 看得到状态

### 现在怎样

`Loader::status()` 返回一份快照：每一行的状态和原因（在等什么、为什么失败），每个服务由哪一行提供（宿主提供的写 `host`），每个运行时的状态和进程号，以及进行中的调用（谁调用、哪个服务的哪个方法、多久了）。它实现 `Display`，一项一行，直接打印（第 1 节「等就绪」里就是）；字段给程序读：

```rust,no_run
# fn demo(handle: rutis_loader::Loader) {
let status = handle.status();
for row in status.rows.iter().filter(|row| row.reason.is_some()) {
    eprintln!("jot: {} is not running: {}", row.id, row.reason.as_deref().unwrap_or(""));
}
for call in &status.calls {
    eprintln!("jot: {call}");                         // "<调用方> → <服务>.<方法>  12s"
}
# }
```

- 与 `rutis-host status` 打印的是同一份（格式见 [rutis-host：查看当前状态](rutis-host.md#查看当前状态)）。
- 看不到的：同一个运行时里的调用、按引用传过来的函数的调用（只看得到「宿主 → 行」）；进程内的 Rust 插件发起的调用记为 `host`。
- 插件和服务的完整情况（包括 rutis 自己的插件）：`root.diagnostics()`，见 [调试](rust-host.md#调试)。

### 随变化做事

要在变化发生时做事（比如一个命令的插件失败时告诉用户），在根上监听 `rutis/row` 与 `rutis/runtime`：

```rust,no_run
# use rutis::host::CtxHost;
# fn demo(root: &rutis::Ctx) -> Result<(), Box<dyn std::error::Error>> {
use rutis_bridge::events::{RuntimeEvent, RuntimeEventState};
use rutis_loader::report::RowEventState;
use rutis_loader::RowEvent;

root.on::<RowEvent>(|event, _| async move {
    if event.state == RowEventState::Failed {
        eprintln!("jot: {} failed: {}", event.row, event.error.as_deref().unwrap_or(""));
    }
    Ok(None)
})?;
root.on::<RuntimeEvent>(|event, _| async move {
    if event.state == RuntimeEventState::Down {
        eprintln!("jot: runtime {} ended: {}", event.runtime, event.reason.as_deref().unwrap_or(""));
    }
    Ok(None)
})?;
# Ok(())
# }
```

- `RowEvent`：行 `row` 现在是 `pending`、`running`、`failed`（`error` 写明原因）或 `stopped`。`RuntimeEvent`：运行时 `runtime` 起来了（`running`，每次重连后也发）或结束了（`down`，`reason` 写明怎样结束）。
- 它们是总线上的普通事件，各语言的插件也收得到；插件不能发出 `rutis/*`。事件的其余写法见 [Rust 宿主：事件](rust-host.md#事件)。
