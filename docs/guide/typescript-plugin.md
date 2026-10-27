# 写一个 TypeScript 插件

从创建项目到发布，再到被宿主使用。JavaScript 写法相同，去掉类型即可。

## 1. 创建项目

<!-- example: typescript run -->
```bash
npx @arcships/rutis-host new weather --lang node
cd weather
npm install
```

得到的项目：

| 文件 | 作用 |
| --- | --- |
| `src/index.ts` | 插件本身 |
| `test/index.test.ts` | 单元测试，不需要宿主 |
| `rutis.dev.json` | 本地运行时的配置和测试用的其他插件 |
| `package.json` | 依赖 `@arcships/rutis`（`^` 范围，见 §5）；开发依赖 `@arcships/rutis-host`、`@arcships/rutis-runtime`；`npm pack`、`npm publish` 前先构建（`prepack`） |
| `.github/workflows/publish.yml` | 打 `v*` tag 时测试并发布到 npm |

## 2. 写插件

生成的 `src/index.ts` 是一个不用任何服务的 `greeter`，`test/index.test.ts` 是它的测试，`npm test` 直接能过。下面把它换成一个用到 `llm` 服务的 `weather`：`src/index.ts` 换成这段，`test/index.test.ts` 同时换成 §3 的那段（两个文件一起换；只换一个，模板自带的测试会失败）。

<!-- example: typescript file=weather/src/index.ts -->
```ts
import { definePlugin } from '@arcships/rutis'

export interface Config {
  city?: string
}

interface Llm {
  ask(question: string): Promise<string>
}

class Weather {
  constructor(private readonly llm: Llm, private readonly city: string) {}

  async today(): Promise<string> {
    return `${await this.llm.ask(`weather in ${this.city}`)} in ${this.city}`
  }
}

export default definePlugin<Config>({
  inject: ['llm'],                                 // 用到的服务：都就绪才启动，任何一个撤销就停下
  provides: { weather: { today: 'async' } },      // 提供的服务：check 运行前就知道；只有声明的方法能调用
  config: { type: 'object', properties: { city: { type: 'string' } } },  // 配置的 JSON Schema
  apply(ctx, config) {
    ctx.provide('weather', new Weather(ctx.use<Llm>('llm'), config.city ?? 'Oslo'))
    return () => {}                                // 清理；也可以用 ctx.effect(cleanup)，或不返回
  },
})
```

插件只依赖 `@arcships/rutis`。它不认识宿主是谁、`llm` 由哪种语言或哪台机器提供，也不管自己什么时候启动和停止：这些由宿主决定。能做什么、值怎样传递，见 [插件 API](plugin-api.md)。

`provides` 写出插件提供什么，`rutis-host check` 不运行插件就知道 `weather` 由这一行提供：宿主在 CI 里用 `check --strict` 检查配置时，用到 `weather` 的行才能通过（模板默认就写了）。写了的服务按声明：只有声明的方法能调用，加方法时也加到这里；`'sync'` 把方法标成同步。不写也能运行：`ctx.provide` 提供的服务照样自动对外，方法取对象上的函数，都按 `async` 调用，只是 `check` 要等插件运行才知道，`--strict` 会报错。

插件本身也是一个值：把 `definePlugin(...)` 的结果照常作为参数交给一个服务（比如应用的部件登记处），收的一方就能在别处挂它，每挂一次是一份，用的是你在 `apply` 里准备好的东西。见 [插件 API](plugin-api.md#插件定义)。

## 3. 测试

`@arcships/rutis/testing` 不需要宿主，给插件提供假服务、调用它提供的服务：

<!-- example: typescript file=weather/test/index.test.ts -->
```ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { load } from '@arcships/rutis/testing'
import plugin from '../src/index.ts'

test('reports the weather of the configured city', async () => {
  const t = await load(plugin, {
    config: { city: 'Oslo' },
    services: { llm: { ask: async () => 'sunny' } },
  })
  assert.equal(await t.service('weather').today(), 'sunny in Oslo')
  await t.unload()
})
```

<!-- example: typescript run -->
```bash
npm test
```

`load` 会检查宿主会检查的事：插件只用 `inject` 里声明的服务；提供的服务带着 `provides` 里声明的每个方法（没有声明的服务，像 rutis 一样按对象上的方法、都按 async 调用；`t.methods(name)` 返回 rutis 看到的方法）；卸载时清理函数都运行了。默认的严格模式下，值按跨进程的规则传递：数据被复制，函数和带方法的对象按引用传递，声明为 `sync` 的方法不能返回 Promise。只在同一进程里才成立的写法，在这里就会失败。

### 事件

`ctx.on(name, listener)` 监听事件；`ctx.emit`、`ctx.parallel`、`ctx.serial`、`ctx.waterfall`、`ctx.bail` 是发出的五种方式。监听器自动登记给宿主，什么都不用声明：发出方是同一个进程里的另一个插件、另一种语言的插件、宿主里的 Rust 代码还是相连的节点，结果都一样。完整的规则见 [插件 API：事件](plugin-api.md#事件)。

下面两个插件在测试工具的 `host()` 上共享一条总线：`greeter` 用 `serial` 取第一个结果、用 `waterfall` 让别的插件改写问候语、用 `emit` 通知；`shout` 是 waterfall 的一环，并监听通知。

<!-- example: typescript file=weather/test/events.test.ts order=2 -->
```ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { definePlugin } from '@arcships/rutis'
import { host } from '@arcships/rutis/testing'

const greeter = definePlugin({
  apply(ctx) {
    // 返回 undefined / null / false 是"没有结果"，serial 接着问下一个监听器
    ctx.on('greeting/pick', (lang: string) => (lang === 'sv' ? 'Hej' : undefined))
    ctx.provide('greeter', {
      async hello(name: string, lang: string) {
        const word = (await ctx.serial<string>('greeting/pick', lang)) ?? 'Hello'
        // 参数都是事件的；终态用 `with` 给出：链的最里面
        const text = await ctx.waterfall.with((text: string) => text)<string>('greeting/format', `${word}, ${name}`)
        ctx.emit('greeting/said', name)                  // 立即返回，不等监听器
        return text
      },
    })
  },
})

const said: string[] = []
const shout = definePlugin({
  apply(ctx) {
    // waterfall 的监听器最后一个参数是 next：执行链上其余部分
    ctx.on('greeting/format', async (text: string, next: () => Promise<string>) => `${await next()}!`)
    ctx.on('greeting/said', (name: string) => { said.push(name) })
  },
})

test('plugins hear each other', async () => {
  const h = host()
  const t = await h.load(greeter)
  await h.load(shout)
  assert.equal(await t.service('greeter').hello('Ada', 'sv'), 'Hej, Ada!')
  assert.equal(await t.service('greeter').hello('Bo', 'en'), 'Hello, Bo!')
  assert.deepEqual(said, ['Ada', 'Bo'])
  assert.equal(await h.serial('greeting/pick', 'sv'), 'Hej')   // 测试自己也能发出
  await h.unload()
})
```

- `emit` 立即返回；监听器出错不影响发出方和别的监听器，在监听器所在处报告一行（测试里在 `h.reported` / `t.reported`）。要等全部监听器、拿到它们的错误，用 `await ctx.parallel(...)`：失败是 `AggregateError`，按登记顺序。
- `waterfall` 只要有监听器在别的进程就返回 Promise，所以总是 `await` 它。`bail` 是同步的 `serial`，遇到 `async` 监听器以 `AsyncListener` 失败。
- `ctx.filter(d => d.row === 'audit').parallel(...)` 只送达过滤函数留下的监听器；`ctx.on(name, fn, { prepend: true })` 排到最前，`{ global: true }` 不受过滤。
- 作用域：`h.load(plugin, { scope: 't1/s1' })` 把插件加载到一个实例里，它的监听器把作用域作为描述带上；监听器收到这个 root 总线上的所有同名事件，与发出方在哪个实例无关。作用域只是描述，供过滤使用。
- 卸载时插件的 `ctx.signal` 被中止，正在运行的监听器被等待（最多到关闭截止时间），然后才运行清理。

`load(plugin)` 返回的 `t` 也有这五个方法和 `filter`，从这个插件的作用域发出。别的语言的插件监听 TypeScript 插件发出的事件，写法见 [Python 插件：事件](python-plugin.md#事件)。

## 4. 在本地宿主里运行

<!-- example: typescript run until="weather: running" order=99 -->
```bash
npx rutis-host dev
```

`dev` 把这个项目作为一行运行（源码 `src/index.ts`，不需要先构建），改了它的文件就重新加载它；只有文件变了的行重新加载，`rutis.dev.json` 改了也在运行中生效（见 [用 dev 开发插件](rutis-host.md#用-dev-开发插件)）。插件需要的其他服务写在 `rutis.dev.json` 里，例如一个假的 `llm`：

<!-- example: typescript file=weather/rutis.dev.json -->
```json
{
  "rows": [
    { "id": "weather", "config": { "city": "Oslo" } },
    { "id": "llm", "name": "./dev/fake-llm.ts" }
  ]
}
```

<!-- example: typescript file=weather/dev/fake-llm.ts -->
```ts
// dev/fake-llm.ts
import { definePlugin } from '@arcships/rutis'
export default definePlugin({
  provides: { llm: { ask: 'async' } },
  apply(ctx) { ctx.provide('llm', { ask: async () => 'sunny' }) },
})
```

和插件 id 相同的那一行给插件本身加配置；其他行是一起运行的插件。`npx rutis-host check` 列出每一行的依赖、提供的服务和配置 Schema，有问题时以非零状态退出，适合放进 CI。

## 5. 发布

```bash
npm publish --access public   # 先运行 prepack：tsc 生成 dist/
```

或者推一个 `v0.1.0` 这样的 tag，模板里的工作流会测试、`check` 并发布（仓库里需要 `NPM_TOKEN` secret）。`npm pack` 和 `npm publish` 都会先运行 `prepack`（`npm run build`），所以包里总有 `dist/`。

发布前给包起一个你自己的名字：`weather`、`greeter` 这样的短名字在 npm 上早被占用了。用带作用域的名字，改 `package.json` 的 `name`，例如 `"@your-name/weather"`（作用域是你的 npm 用户名或组织；首次发布带作用域的公开包要 `--access public`，模板的工作流已经带上）。宿主的行名就是这个包名。

依赖写 `"@arcships/rutis": "^0.17.2"` 这样的范围（`rutis-host new` 写的是它自己的版本）。插件用的是它写时那个次版本的 SDK：宿主里装着别的版本时，npm 给插件装它自己的一份，几个版本的 SDK 可以并存（插件靠 `Symbol.for` 的标记被认出，与是哪一份 SDK 无关）。插件上带有它所用的插件 API 版本（`definePlugin` 自动标记），宿主的运行时比它旧时会明确报错，而不是加载后出错。

## 6. 被宿主使用

宿主把插件包装进它的 Node 项目（和 `@arcships/rutis-runtime` 同一处），然后在配置里加一行，行名就是包名：

```bash
npm install @your-name/weather
```

```json
{ "id": "weather", "name": "@your-name/weather", "config": { "city": "Oslo" } }
```

见 [rutis-host 与 rutis.json](rutis-host.md#宿主怎样装插件)；Rust 宿主见 [在 Rust 应用里嵌入](rust-host.md)。

## 已有的 Cordis 插件

不用改写，也不用改包：`rutis.json` 的行里写包名（已发布的包也一样）：

```json
{ "id": "storage", "name": "@deepseek-ai/dsh-storage" }
```

它在 Node 运行时的 Cordis Context 里运行，和叶子插件、其他 Cordis 行在同一个进程：同一个运行时里的行按 Cordis 的原生规则互相使用服务，拿到对象本身。它提供的服务自动投到 rutis，别的运行时和 Rust 按名字使用，方法取服务对象上的函数，按 `async` 调用。要把某些方法标成 `sync`，或者让 `rutis-host check` 在运行前就知道它提供什么，在包的 `package.json` 里声明（声明了就按声明）：

```json
"rutis": { "provides": { "weather": { "today": "sync" } } }
```

重名的规则、`check` 怎样报告见 [Cordis](cordis.md)。
