# 写一个 Bun 插件

插件在 [Bun](https://bun.com) 里运行：宿主启动 Bun 运行时 `@arcships/rutis-bun`，TypeScript 不需要编译。写法与 [TypeScript 插件](typescript-plugin.md) 相同，命令换成 `bun` 的。下面 8 步从生成项目到发布，例子是模板生成的 `greeter`：它提供 `greeter` 服务，用别人提供的 `translate` 服务把问候语翻译好。需要 Bun 1.4 或更高（Windows 尚未测试）。

## 1. 起项目

<!-- example: bun run -->
```bash
bunx --bun @arcships/rutis-host new greeter --lang bun
cd greeter
bun install
bun test
```

`new` 打印：

```text
created greeter/
next: cd greeter && bun install && bun test && bunx --bun rutis-host dev
```

| 文件 | 作用 |
| --- | --- |
| `src/index.ts` | 插件本身 |
| `test/index.test.ts` | 测试（`bun test`），不需要宿主 |
| `rutis.dev.json` | 本地运行时的行（第 7 步） |
| `package.json` | 依赖 `@arcships/rutis`；开发依赖 `@arcships/rutis-bun`、`@arcships/rutis-host` |
| `tsconfig.json` | `bun run check` 做类型检查：Bun 运行 TypeScript 时不检查类型 |
| `.github/workflows/publish.yml` | 打 `v*` tag 时测试并发布到 npm（第 8 步） |

## 2. 提供服务

生成的 `src/index.ts` 就是一个插件：

<!-- example: bun file=greeter/src/index.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

// Each part below is a step of the Bun plugin guide:
// https://github.com/arcships/rutis/blob/main/docs/guide/bun-plugin.en.md

export interface Config {
  // Filled from the schema's default when the row leaves it out (step 4).
  greeting: string
}

class Greeter {
  constructor(private readonly greeting: string) {}

  hello(name: string): string {
    return `${this.greeting}, ${name}!`
  }
}

export default definePlugin<Config>({
  // Services this plugin uses: `inject: ['translate']`, then
  // `ctx.use('translate')` (step 3).
  inject: [],
  // The services it provides, with their methods (step 2).
  provides: { greeter: { hello: 'async' } },
  // The JSON Schema of its config (step 4).
  config: { type: 'object', properties: { greeting: { type: 'string', default: 'Hello' } } },
  apply(ctx, config) {
    ctx.provide('greeter', new Greeter(config.greeting))
  },
})
```

- `definePlugin` 定义插件；宿主运行它时调用 `apply`。
- `ctx.provide('greeter', 对象)` 提供名为 `greeter` 的服务：别的插件、别的语言、宿主都按这个名字调用它的方法。
- `provides` 写出提供哪些服务、各有哪些方法，宿主不运行插件就知道。加方法时也加到这里。

## 3. 使用服务

`greeter` 要用 `translate` 服务翻译问候语。在 `inject` 里写上它，用 `ctx.use` 拿到：

<!-- example: bun file=greeter/src/index.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

export interface Config {
  greeting: string
}

interface Translate {
  text(text: string): Promise<string>
}

class Greeter {
  constructor(private readonly translate: Translate, private readonly greeting: string) {}

  async hello(name: string): Promise<string> {
    return `${await this.translate.text(this.greeting)}, ${name}!`
  }
}

export default definePlugin<Config>({
  inject: ['translate'],
  provides: { greeter: { hello: 'async' } },
  config: { type: 'object', properties: { greeting: { type: 'string', default: 'Hello' } } },
  apply(ctx, config) {
    ctx.provide('greeter', new Greeter(ctx.use<Translate>('translate'), config.greeting))
  },
})
```

`inject` 里的服务都就绪了，插件才启动；其中一个撤销，插件就停下。`translate` 由哪种语言、哪一行提供，插件不用管。模板自带的测试没有给 `translate`，第 6 步跟着改。

## 4. 配置

插件的配置是 `apply` 的第二个参数。`config` 是它的 JSON Schema：`greeting` 的 `default` 是 `'Hello'`，行里没写 `greeting` 时，`apply` 收到的就是 `{ greeting: 'Hello' }`。不合 Schema 的配置让这一行以 `config: …` 失败，不调用 `apply`。

配置从哪里来：测试里由 `load` 的 `config` 传入（第 6 步）；运行时来自宿主里这一行的 `config`（第 7 步的 `rutis.dev.json`）。

## 5. 发出和监听事件

每问候一个人，`greeter` 发出事件 `greeter/greeted`；它自己也监听这个事件，记下问候了几次，用新方法 `count()` 返回：

<!-- example: bun file=greeter/src/index.ts -->
```ts
import { definePlugin, type Context } from '@arcships/rutis'

export interface Config {
  greeting: string
}

interface Translate {
  text(text: string): Promise<string>
}

class Greeter {
  greeted = 0

  constructor(
    private readonly ctx: Context,
    private readonly translate: Translate,
    private readonly greeting: string,
  ) {}

  async hello(name: string): Promise<string> {
    const greeting = await this.translate.text(this.greeting)
    this.ctx.emit('greeter/greeted', name)
    return `${greeting}, ${name}!`
  }

  async count(): Promise<number> {
    return this.greeted
  }
}

export default definePlugin<Config>({
  inject: ['translate'],
  provides: { greeter: { hello: 'async', count: 'async' } },
  config: { type: 'object', properties: { greeting: { type: 'string', default: 'Hello' } } },
  apply(ctx, config) {
    const greeter = new Greeter(ctx, ctx.use<Translate>('translate'), config.greeting)
    ctx.on('greeter/greeted', () => { greeter.greeted++ })
    ctx.provide('greeter', greeter)
  },
})
```

- `ctx.emit(名字, ...参数)` 发出事件，`ctx.on(名字, 监听器)` 监听。监听器不用声明：别的插件、别的语言、宿主发出的同名事件都送到这里。插件卸载时监听器自动撤掉。
- 其余发出方式（等结果、按顺序、取第一个结果等）见 [插件 API：事件](plugin-api.md#事件)。

## 6. 测试

`@arcships/rutis/testing` 的 `load` 在真实的宿主上装载插件（`rutis-host test`，项目的开发依赖里已经有），插件运行在这个 Bun 运行时里：传入配置和假的 `translate`，取服务调用，监听插件发出的事件，最后卸载。`test/index.test.ts` 换成：

<!-- example: bun file=greeter/test/index.test.ts -->
```ts
import { expect, test } from 'bun:test'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

const words: Record<string, string> = { Hello: 'Bonjour', Hi: 'Salut' }

test('greets with the translated greeting', async () => {
  const t = await load(plugin, {
    config: { greeting: 'Hi' },
    services: { translate: { text: async (text: string) => words[text] ?? text } },
  })
  const greeted: string[] = []
  t.on('greeter/greeted', (name: string) => { greeted.push(name) })
  expect(await t.service('greeter').hello('Ada')).toBe('Salut, Ada!')
  expect(await t.service('greeter').count()).toBe(1)
  await t.unload()
  expect(greeted).toEqual(['Ada'])
})
```

<!-- example: bun run -->
```bash
bun test
bun run check
```

- `services` 给插件 `inject` 的服务；`t.service(名字)` 是插件提供的服务；`t.on` 像插件旁边的另一个插件一样监听事件（经宿主送达，比服务的返回晚一点到，所以断言放在 `await t.unload()` 之后）；`t.unload()` 卸载插件，运行它的清理。
- 宿主做它在真实环境里做的事：按 Schema 检查配置并补上默认值、分发事件。值在插件与测试之间跨进程传递。插件要是它那个文件的默认导出。
- 宿主给了服务的类型（`rutis-host types` 写出的 `.d.ts`）时，写 `ctx.use<Services['translate']>('translate')`，`bun run check` 在运行前检查调用，见 [做一个开放插件的宿主：给插件作者的类型](open-host.md#给插件作者的类型)。
- 测试工具的其余接口见 [插件 API：TypeScript 与 Bun 的测试工具](plugin-api.md#typescript-与-bun-的测试工具)。

## 7. 本地运行并试一下

`rutis-host dev` 把这个项目放进一个本地宿主里运行。宿主运行的每个插件是一**行**，`rutis.dev.json` 写出这些行：

<!-- example: bun file=greeter/rutis.dev.json -->
```json
{
  "format": 1,
  "rows": [
    { "id": "greeter", "config": { "greeting": "Hello" } },
    { "id": "translate", "name": "bun:./dev/fake-translate.ts" }
  ]
}
```

- 和插件同 id 的那一行（`greeter`）就是插件自己，`config` 是它的配置。
- 另一行是假的 `translate`：`name` 写出插件在哪里，`bun:` 表示在 Bun 运行时里运行，路径相对于项目。

假的 `translate` 也是一个 Bun 插件：

<!-- example: bun file=greeter/dev/fake-translate.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

const words: Record<string, string> = { Hello: 'Bonjour', Hi: 'Salut' }

export default definePlugin({
  provides: { translate: { text: 'async' } },
  apply(ctx) {
    ctx.provide('translate', { text: async (text: string) => words[text] ?? text })
  },
})
```

<!-- example: bun run until="greeter: running" -->
```bash
bunx --bun rutis-host dev
```

```text
rutis-host dev: running greeter; changes reload it (Ctrl-C ends)
greeter: starting
translate: running
rutis-host dev: services: greeter (greeter), loader (host), sessions (host), translate (translate)
greeter: running
```

`greeter` 等 `translate` 就绪后才启动。`services` 那一行是现在的服务和提供它的行；`loader` 是宿主自己提供的。这一行在所有行都已运行、等待或失败后打印一次，之后服务变化时再打印，和各行状态行的先后不固定。

`dev` 运行时，另开一个终端，在项目目录里看现在有哪些服务、由哪一行提供，再调一次 `greeter.hello`：

```console
$ bunx --bun rutis-host status
rows:
  greeter: running
  translate: running
services:
  greeter: greeter
  loader: host
  sessions: host
  translate: translate
runtimes:
  bun: running (pid 66997)
calls:
  none
$ bunx --bun rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!"
$ bunx --bun rutis-host call greeter.count
1
```

`call` 的每个参数是一个 JSON 值，字符串带引号（`'"Ada"'`）；结果打印成 JSON，出错时写到标准错误并以 1 退出。见 [rutis-host：查看当前状态](rutis-host.md#查看当前状态)、[调用一个服务](rutis-host.md#调用一个服务)。

改 `src/index.ts` 并保存（比如把 `${name}!` 改成 `${name}.`），`dev` 重新加载这一行，再 `call` 就是新代码的结果：

```text
greeter: reloaded (src/index.ts changed)
rutis-host dev: services: greeter (greeter), loader (host), sessions (host), translate (translate)
```

```console
$ bunx --bun rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada."
```

只有文件变了的行重新加载；`rutis.dev.json` 改了也在运行中生效。见 [rutis-host：用 dev 开发插件](rutis-host.md#用-dev-开发插件)。

在另一台机器上运行见 [rutis-host.md 的远程一节](rutis-host.md#远程运行时与设备)。

## 8. 发布

- **包名**：发布前改 `package.json` 的 `name`，用带作用域的名字（`"@your-name/greeter"`；`greeter` 这样的短名字在 npm 上早被占用了）。
- **打 tag 发布**：推一个 `v0.1.0` 这样的 tag，模板的工作流运行 `bun test`、`bun run check`、`rutis-host check`，再 `npm publish`（仓库里要有 `NPM_TOKEN` secret）。手动发布用 `npm publish --access public`，它先运行 `prepack` 构建 `dist/`。
- **依赖范围**：`@arcships/rutis` 写 `^` 范围（`rutis-host new` 写的是它自己的版本），宿主里几个版本的 SDK 可以并存。

宿主怎样装你的插件：[rutis-host：宿主怎样装插件](rutis-host.md#宿主怎样装插件)。

## 调试

`--debug` 打开 Bun 运行时进程自己的检查器，地址固定为 `ws://127.0.0.1:6499/rutis`：

```bash
bunx --bun rutis-host dev --debug
```

在浏览器里打开输出里的 `https://debug.bun.sh/#127.0.0.1:6499/rutis`，或者用 VS Code 的 Bun 扩展 attach 到 `ws://127.0.0.1:6499/rutis`。在源文件里下断点，或者在代码里写 `debugger;`。

- 地址每次启动都一样，运行时重启后重新连上即可。
- 停在断点时整个 Bun 运行时停着：这个运行时里其他行的调用也等它继续。
- `console.log` 的内容出现在 `dev` 的输出里。

## 运行时的约定

- **只用项目里装好的包**：运行时总是带 `--no-install` 启动，缺少的包会让加载失败，不会被下载。用 `bun add` 安装插件和它的依赖。`bun install` 默认只为 Bun 内置受信名单里的依赖运行安装脚本；项目里写 `trustedDependencies` 会**替换**这个名单，只列确实需要安装脚本的包。
- **不读 `.env`**：运行时带 `--no-env-file` 启动，环境变量只来自宿主。项目的 `bunfig.toml` 照常生效（包括 `preload`），它是项目自己的可信配置。
- **同步调用可重入**：插件同步调用 rutis 服务、等待结果时，别的调用仍会进入这个运行时执行。所以插件的服务可能在它自己的同步调用中途被调用：调用 rutis 服务时不要持有锁。
- **未捕获的错误结束进程**：这个运行时的所有服务随之撤销。
- **重新加载**只重新导入插件模块本身；它导入的其他模块要重启运行时才更新。
- **运行期间新装的包**（`bun add`）要重启运行时才能被找到：Bun 进程会记住某个包曾经不存在。
- `rutis-host dev` 看到 `bun.lock`、`rutis.dev.json` 里的 `runtimes.bun`，或依赖 `@arcships/rutis-bun`，就用 Bun 运行这个项目。

## 进阶：插件 API

插件作为值、其余事件方式、作用域、同步方法、带方法的对象、测试工具的其余接口：[插件 API](plugin-api.md)。
