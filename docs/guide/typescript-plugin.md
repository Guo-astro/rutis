# 写一个 TypeScript 插件

[English](typescript-plugin.en.md)

从生成项目到发布，八步。全篇是同一个插件 `greeter`：它提供 `greeter` 服务，用别的插件提供的 `translate` 服务把问候语翻译出来。JavaScript 写法相同，去掉类型即可。需要 Node 22 或更高。

## 1. 起项目

<!-- example: typescript run -->
```bash
npx @arcships/rutis-host new greeter --lang node
cd greeter
npm install
npm test
```

得到的项目：

| 文件 | 作用 |
| --- | --- |
| `src/index.ts` | 插件本身 |
| `test/index.test.ts` | 单元测试，不需要宿主 |
| `rutis.dev.json` | 本地运行时的几行（第 7 步） |
| `package.json` | 依赖 `@arcships/rutis`；开发依赖 `@arcships/rutis-host`、`@arcships/rutis-runtime` |
| `.github/workflows/publish.yml` | 打 `v*` tag 时测试并发布到 npm（第 8 步） |

## 2. 提供服务

生成的 `src/index.ts` 就是一个插件：

<!-- example: typescript file=greeter/src/index.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

export interface Config {
  // Filled from the schema's default when the row leaves it out.
  greeting: string
}

class Greeter {
  constructor(private readonly greeting: string) {}

  hello(name: string): string {
    return `${this.greeting}, ${name}!`
  }
}

// The steps of the TypeScript plugin guide:
// https://github.com/arcships/rutis/blob/main/docs/guide/typescript-plugin.en.md
export default definePlugin<Config>({
  // Services this plugin uses (step 3): `inject: ['translate']`, then
  // `ctx.use('translate')`. It starts once they are all available.
  inject: [],
  // The services it provides and their methods (step 2). Only the methods
  // listed here can be called: add each new one.
  provides: { greeter: { hello: 'async' } },
  // The JSON Schema of its config (step 4): the config apply receives has
  // the defaults filled in.
  config: { type: 'object', properties: { greeting: { type: 'string', default: 'Hello' } } },
  apply(ctx, config) {
    ctx.provide('greeter', new Greeter(config.greeting))
  },
})
```

- `definePlugin` 定义插件；模块的默认导出就是它。
- `apply` 在插件启动时运行，`ctx.provide('greeter', 对象)` 把对象作为服务 `greeter` 提供出去。别的插件（任何语言）按名字 `greeter` 使用它，调用它的方法。
- `provides` 写出提供哪些服务、各有哪些方法：只有写在这里的方法能被调用，加方法时也加到这里。别的插件调用时都是异步的（`'async'`），所以 `hello` 返回字符串也可以。
- 插件停止时，它提供的服务随之撤掉。

## 3. 使用服务

`greeter` 改成用 `translate` 服务翻译问候语：`inject` 写上 `translate`，`ctx.use('translate')` 拿到它。把 `src/index.ts` 换成：

<!-- example: typescript file=greeter/src/index.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

export interface Config {
  greeting: string
}

interface Translate {
  text(text: string): Promise<string>
}

class Greeter {
  constructor(
    private readonly translate: Translate,
    private readonly greeting: string,
  ) {}

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

`inject` 里的服务都就绪了插件才启动，其中一个撤掉时插件停下，它回来时插件再启动。`translate` 由哪个插件、哪种语言提供，插件不用管。别的插件的方法调用都是异步的，所以要 `await`。

模板自带的测试这时不再通过（它没有给出 `translate`），第 6 步换掉它。

## 4. 配置

`config` 是配置的 JSON Schema。`apply` 收到的 `config` 已经补上 Schema 里的默认值：没有给出 `greeting` 时它是 `'Hello'`。配置不合 Schema 时插件不启动。

配置从哪里来：测试里由 `load(plugin, { config })` 传入（第 6 步）；运行时来自宿主里这个插件的那一行（第 7 步的 `rutis.dev.json`、宿主的 `rutis.json`）。

## 5. 发出和监听事件

每次问候后发出事件 `greeter/greeted`，参数是名字；`greeter` 自己也监听这个事件，数一数一共问候过几次，用新方法 `count` 返回。`src/index.ts` 换成：

<!-- example: typescript file=greeter/src/index.ts -->
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
    const text = `${await this.translate.text(this.greeting)}, ${name}!`
    this.ctx.emit('greeter/greeted', name)
    return text
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
    ctx.on('greeter/greeted', () => { greeter.greeted += 1 })
    ctx.provide('greeter', greeter)
  },
})
```

- `ctx.emit(名字, ...参数)` 发出事件，马上返回。
- `ctx.on(名字, 监听器)` 监听事件：不论发出方是这个插件、同一进程里的另一个插件、另一种语言的插件还是宿主，都送到。不用声明；插件停止时监听器随之撤掉。
- `count` 是新方法，所以也加进了 `provides`。

其余发出方式（等每个监听器返回的、拿到返回值的）见 [插件 API：事件](plugin-api.md#事件)。

## 6. 测试

`@arcships/rutis/testing` 的 `load` 在真实的宿主上装载插件（`rutis-host test`，项目的开发依赖里已经有）：给插件配置和假的 `translate`，取它提供的服务来调用，监听它发出的事件，最后卸载。`test/index.test.ts` 换成：

<!-- example: typescript file=greeter/test/index.test.ts -->
```ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

const french: Record<string, string> = { Hello: 'Bonjour', Hi: 'Salut' }
const translate = { text: async (text: string) => french[text] ?? text }

test('greets with the translated greeting', async () => {
  const t = await load(plugin, { config: { greeting: 'Hi' }, services: { translate } })
  const greeted: string[] = []
  t.on('greeter/greeted', (name: string) => { greeted.push(name) })
  const greeter = t.service('greeter')
  assert.equal(await greeter.hello('Ada'), 'Salut, Ada!')
  assert.equal(await greeter.count(), 1)
  await t.unload()
  assert.deepEqual(greeted, ['Ada'])
})

test('the greeting is Hello when the config leaves it out', async () => {
  const t = await load(plugin, { services: { translate } })
  assert.equal(await t.service('greeter').hello('Ada'), 'Bonjour, Ada!')
  await t.unload()
})
```

<!-- example: typescript run -->
```bash
npm test
```

- `services` 是插件 `inject` 的服务的假实现，按名字给出。
- `t.service('greeter')` 是插件提供的服务，只有 `provides` 里写的方法能调用，和在宿主里一样。
- `t.on(名字, 监听器)` 像插件旁边的另一个插件一样监听事件，插件发出的都送到。事件经宿主送达，比 `hello` 的返回晚一点到，所以断言放在 `await t.unload()` 之后。
- 插件运行在真实的 Node 运行时里，由真实的宿主管理：宿主按 Schema 检查配置并补上默认值、分发事件，值在插件与测试之间跨进程传递，所以测试过了，在宿主里也一样。插件要是它那个文件的默认导出（`src/index.ts` 就是）。

宿主给了服务的 TypeScript 声明（`.d.ts`）时，用它代替自己写的 `Translate` 接口，例如 `ctx.use<Services['translate']>('translate')`，编辑器和 `tsc` 在运行前检查调用；测试里的假实现照同一个接口写。测试工具的其余接口见 [插件 API：TypeScript 与 Bun 的测试工具](plugin-api.md#typescript-与-bun-的测试工具)。

## 7. 本地运行并试一下

`rutis-host dev` 在本地运行插件，改了文件就重新加载。宿主里每个插件是一「行」：行写出运行哪个插件、给它什么配置。`dev` 运行的几行写在 `rutis.dev.json` 里。

先写一个假的 `translate`，也是一个插件，放在 `dev/` 下：

<!-- example: typescript file=greeter/dev/fake-translate.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

const french: Record<string, string> = { Hello: 'Bonjour', Hi: 'Salut' }

export default definePlugin({
  provides: { translate: { text: 'async' } },
  apply(ctx) {
    ctx.provide('translate', { text: async (text: string) => french[text] ?? text })
  },
})
```

`rutis.dev.json` 换成两行：

<!-- example: typescript file=greeter/rutis.dev.json -->
```json
{
  "format": 1,
  "rows": [
    { "id": "greeter", "config": { "greeting": "Hello" } },
    { "id": "translate", "name": "./dev/fake-translate.ts" }
  ]
}
```

- 和插件同 id 的那一行（`greeter`）就是这个项目的插件自己，`config` 是给它的配置。
- 另一行是假的 `translate`：`name` 写插件在哪里。

<!-- example: typescript run until="rutis-host dev: services: greeter (greeter), loader (host), translate (translate)" -->
```bash
npx rutis-host dev
```

```text
rutis-host dev: running greeter; changes reload it (Ctrl-C ends)
greeter: starting
translate: running
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
greeter: running
```

`greeter` 等 `translate` 就绪后才运行，几行的先后可能和这里不同。`rutis-host dev: services:` 一行是现在的服务和提供它的行（`loader` 由宿主提供），这一行在所有行都已运行、等待或失败后打印一次，之后服务变化时再打印，和各行状态行的先后不固定。`dev` 运行时，另开一个终端，在项目目录里看状态、调一次 `greeter`：

```console
$ npx rutis-host status
rows:
  greeter: running
  translate: running
services:
  greeter: greeter
  loader: host
  translate: translate
runtimes:
  node: running (pid 63145)
calls:
  none
$ npx rutis-host call greeter.hello '"Ada"'
"Bonjour, Ada!"
$ npx rutis-host call greeter.count
1
```

`call` 的每个参数是一个 JSON 值，字符串要带引号；不带时报 `rutis-host: argument 1 is not JSON: write a string in quotes, as '"Ada"'`。

改代码：把 `src/index.ts` 里的 `` `, ${name}!` `` 改成 `` `, ${name}! Welcome.` `` 并保存，`dev` 打印：

```text
greeter: reloaded (src/index.ts changed)
rutis-host dev: services: greeter (greeter), loader (host), translate (translate)
```

再调一次，返回 `"Bonjour, Ada! Welcome."`。只有文件变了的那一行重新加载；`rutis.dev.json` 改了也在运行中生效。Ctrl-C 结束 `dev`。`dev` 的全部规则见 [rutis-host：用 dev 开发插件](rutis-host.md#用-dev-开发插件)。

在另一台机器上运行见 [rutis-host.md 的远程一节](rutis-host.md#远程运行时)。

## 8. 发布

先给包起一个你自己的名字：`greeter` 这样的短名字在 npm 上早被占用了。改 `package.json` 的 `name`，用带作用域的名字，例如 `"@your-name/greeter"`（作用域是你的 npm 用户名或组织）。

```bash
npm publish --access public   # 先运行 prepack：tsc 生成 dist/
```

或者推一个 `v0.1.0` 这样的 tag：模板里的 `.github/workflows/publish.yml` 测试并发布（仓库里需要 `NPM_TOKEN` secret）。

依赖写 `"@arcships/rutis": "^0.18.0"` 这样的范围（`rutis-host new` 写的是它自己的版本）；宿主里装着别的版本时，npm 给插件装它自己的一份，几份可以并存。

宿主怎样装你的插件见 [rutis-host：宿主怎样装插件](rutis-host.md#宿主怎样装插件)。

## 调试

`--debug` 打开 Node 运行时进程自己的检查器，监听 `127.0.0.1:9229`：

<!-- example: typescript run until="greeter: running" -->
```bash
npx rutis-host dev --debug
```

输出里有 `Debugger listening on ws://127.0.0.1:9229/…`。用 Chrome 打开 `chrome://inspect`，在 Remote Target 下点 inspect；或者用 VS Code 的 attach 配置连端口 9229。在 Sources 里打开 `src/index.ts` 下断点，或者在代码里写 `debugger;`。

- 启动 `rutis-host` 的 npm 启动器不占这个端口。不要用 `NODE_OPTIONS=--inspect`：它也给启动器开检查器，先启动的启动器占掉端口，连上的是启动器；`--inspect-brk` 会让启动器停住。
- 重新加载后模块的地址带 `?rutis-reload=<n>`，是一个新的脚本：在新脚本里重新下断点，或者用 `debugger;`。
- 停在断点时整个 Node 运行时停着：这个运行时里其他行的调用也等它继续。
- 端口 9229 已被占用时，Node 打印 `Starting inspector on 127.0.0.1:9229 failed` 并照常运行，没有检查器：先结束占用端口的进程。
- `console.log` 的内容出现在 `dev` 的输出里。

## 进阶：插件 API

插件作为值、其余事件方式、带方法的对象、清理、同步方法、版本等见 [插件 API](plugin-api.md)；已有的 Cordis 插件不用改写，见 [Cordis](cordis.md)。
