# 写一个 Bun 插件

插件在 [Bun](https://bun.com) 里运行：宿主启动 Bun 运行时 `@arcships/rutis-bun`，行名写作 `bun:<模块>`。插件的写法与 [TypeScript 插件](typescript-plugin.md)相同（`definePlugin`，见 [插件 API](plugin-api.md)），区别在于由 Bun 运行，TypeScript 不需要编译。

## 1. 创建项目

```bash
bunx --bun @arcships/rutis-host new greeter --lang bun
cd greeter
bun install
```

| 文件 | 作用 |
| --- | --- |
| `src/index.ts` | 插件本身 |
| `test/index.test.ts` | 单元测试（`bun test`），不需要宿主 |
| `rutis.dev.json` | 本地运行时的配置和测试用的其他插件 |
| `package.json` | 依赖 `@arcships/rutis`（`^` 范围，见 [TypeScript 插件 §5](typescript-plugin.md#5-发布)）；开发依赖 `@arcships/rutis-bun`、`@arcships/rutis-host`；`npm pack`、`npm publish` 前先构建（`prepack`） |
| `tsconfig.json` | `bun run check` 做类型检查：Bun 运行 TypeScript 时不检查类型 |
| `.github/workflows/publish.yml` | 打 `v*` tag 时测试并发布到 npm |

## 2. 写插件与测试

写法与 TypeScript 插件第 2、3 节相同。测试用 `bun test`：

```ts
import { expect, test } from 'bun:test'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

test('greets', async () => {
  const t = await load(plugin, { config: { greeting: 'Hi' } })
  expect(await t.service('greeter').hello('Ada')).toBe('Hi, Ada!')
  await t.unload()
})
```

不用 SDK 时，模块也可以直接导出 `apply(ctx, config)`，以及 `inject`、`provides`（可选，每个方法写明 `'sync'` 或 `'async'`）、`config`（JSON Schema）。`provides` 不写也行：`ctx.provide` 提供的服务自动对外，方法取对象上的函数，按 `async` 调用，规则和 Node 相同（见 [插件 API](plugin-api.md)）。

`definePlugin(...)` 的结果也可以照常作为参数交给一个服务，收的一方在别处挂它，和 Node 相同，见 [插件 API](plugin-api.md#插件定义)。

### 事件

与 TypeScript 插件相同（[TypeScript 插件：事件](typescript-plugin.md#事件)）：`ctx.on` 监听，`ctx.emit` 发出。监听器自动登记给宿主，Node、Python、Go 插件和宿主里的 Rust 代码发出的事件同样到达 Bun 里的监听器，反过来也一样。其余发出方式和一个完整的例子见 [插件 API：事件](plugin-api.md#bun)。

测试要确认插件发出了某个事件，用 `t.on(name, listener, options)` 像宿主一样在根作用域监听，与 [TypeScript 插件：测试](typescript-plugin.md#3-测试) 相同：插件发出的、测试自己发出的都送到，返回撤销函数。

### 带方法的对象

类的实例、有函数属性的普通对象按引用交给别的语言，对方拿到代理：见 [插件 API：带方法的对象](plugin-api.md#带方法的对象)。

## 3. 在宿主里运行

```bash
bunx --bun rutis-host dev
```

`rutis-host dev` 看到 `bun.lock`、`rutis.dev.json` 里的 `runtimes.bun`，或依赖 `@arcships/rutis-bun`，就用 Bun 运行这个项目，它的文件变化时重新加载；只有文件变了的行重新加载（见 [用 dev 开发插件](rutis-host.md#用-dev-开发插件)）。

宿主怎样装这个插件见 [rutis-host：宿主怎样装插件](rutis-host.md#宿主怎样装插件)。

### 调用一个方法

`dev` 启动后和每次重新加载后，打印现在有哪些服务、各由哪一行提供：`rutis-host dev: services: greeter (greeter), loader (host)`。

`dev` 运行时，另开一个终端，在项目目录里调用插件提供的服务，看它返回什么、出错时报什么，不用另写一个调用它的插件：

```console
$ bunx --bun rutis-host call greeter.hello '"Ada"' '"en"'
"Hello, Ada!"
```

每个参数是一个 JSON 值（字符串带引号：`'"Oslo"'`），结果打印成 JSON；错误写到标准错误并以 1 退出。见 [rutis-host：调用一个服务](rutis-host.md#调用一个服务)。

### 调试

`--debug` 打开 Bun 运行时进程自己的检查器，地址固定为 `ws://127.0.0.1:6499/rutis`：

```bash
bunx --bun rutis-host dev --debug
```

在浏览器里打开输出里的 `https://debug.bun.sh/#127.0.0.1:6499/rutis`，或者用 VS Code 的 Bun 扩展 attach 到 `ws://127.0.0.1:6499/rutis`。在源文件里下断点，或者在代码里写 `debugger;`。

- 地址每次启动都一样，运行时重启后重新连上即可。
- 停在断点时整个 Bun 运行时停着：这个运行时里其他行的调用也等它继续。
- `console.log` 的内容出现在 `dev` 的输出里。

## 4. 运行时的约定

- **只用项目里装好的包**：运行时总是带 `--no-install` 启动，缺少的包会让加载失败，不会被下载。用 `bun add` 安装插件和它的依赖。`bun install` 默认只为 Bun 内置受信名单里的依赖运行安装脚本；项目里写 `trustedDependencies` 会**替换**这个名单，只列确实需要安装脚本的包。
- **不读 `.env`**：运行时带 `--no-env-file` 启动，环境变量只来自宿主。项目的 `bunfig.toml` 照常生效（包括 `preload`），它是项目自己的可信配置。
- **同步调用可重入**：插件同步调用 rutis 服务、等待结果时，别的调用仍会进入这个运行时执行。所以插件的服务可能在它自己的同步调用中途被调用：调用 rutis 服务时不要持有锁。
- **未捕获的错误结束进程**：这个运行时的所有服务随之撤销。
- **重新加载**只重新导入插件模块本身；它导入的其他模块要重启运行时才更新。
- **运行期间新装的包**（`bun add`）要重启运行时才能被找到：Bun 进程会记住某个包曾经不存在。

## 5. 发布

与 TypeScript 插件相同：`npm publish`（先运行 `prepack` 构建；模板的 workflow 在打 `v*` tag 时发布；包名用带作用域的名字，见 [TypeScript 插件 §5](typescript-plugin.md#5-发布)）。宿主怎样装它见 [rutis-host：宿主怎样装插件](rutis-host.md#宿主怎样装插件)。

## 6. 在别的机器上运行

Bun 运行时也可以在另一台机器上运行，由那里的 rutis 管理它的插件，两个方向都可以（见 [连接节点](nodes.md)）：

```bash
# 监听，由 rutis 来连（rutis 一侧写节点行与 runtimes.remote，language 为 bun）
RUTIS_TOKEN=secret RUTIS_CERT=server.pem RUTIS_KEY=server.key \
  bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts listen:wss://0.0.0.0:7443/rutis --id edge --peer main .
# 拨出，连到 rutis 的接入点（rutis 一侧写 runtimes.remote 的 listen 与 instances）
RUTIS_TOKEN=secret RUTIS_CA=ca.pem \
  bun --no-install --no-env-file node_modules/@arcships/rutis-bun/src/main.ts dial:wss://main.example.com:7443/rutis --id worker-1 --peer main .
```

- 插件从最后一个参数（项目目录）解析；行名写作 `<运行时名>:<模块>`，如 `edge:greeter`、`edge:./report.ts`。
- 每个会话由一个子运行时进程运行：会话结束（断开、被新的连接接管）时它卸载全部行后退出，下一个会话起新的进程。
- 拨出按固定规则重连，状态写在 stderr（`rutis: connected to …` 等），SIGTERM 或 Ctrl-C 卸载全部行后退出，规则见 [连接节点](nodes.md#连进来的运行时)。`RUTIS_CA` 与 Bun 默认信任的根证书一起信任。

## 环境

Bun 1.4 或更高。Windows 尚未测试。
