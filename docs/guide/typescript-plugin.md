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
  city: string
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
  config: { type: 'object', properties: { city: { type: 'string', default: 'Oslo' } } },  // 配置的 JSON Schema
  apply(ctx, config) {
    ctx.provide('weather', new Weather(ctx.use<Llm>('llm'), config.city))
    return () => {}                                // 清理；也可以用 ctx.effect(cleanup)，或不返回
  },
})
```

`apply` 之前，rutis 给配置补上 Schema 里的默认值（行里不写 `city` 时就是 `Oslo`）并校验：配置不合格的行以 `config: …` 失败，不调用 `apply`（[插件 API：配置](plugin-api.md#配置)）；测试工具也一样。

插件只依赖 `@arcships/rutis`。它不认识宿主是谁、`llm` 由哪种语言或哪台机器提供，也不管自己什么时候启动和停止：这些由宿主决定。能做什么、值怎样传递，见 [插件 API](plugin-api.md)。

`provides` 写出插件提供什么，`rutis-host check` 不运行插件就知道 `weather` 由这一行提供：宿主在 CI 里用 `check --strict` 检查配置时，用到 `weather` 的行才能通过（模板默认就写了）。写了的服务按声明：只有声明的方法能调用，加方法时也加到这里；`'sync'` 把方法标成同步。不写也能运行：`ctx.provide` 提供的服务照样自动对外，方法取对象上的函数，都按 `async` 调用，只是 `check` 要等插件运行才知道，`--strict` 会报错。

插件本身也是一个值，可以交给服务、在别处挂很多次：见 [插件 API：插件定义](plugin-api.md#插件定义)。

### 带方法的对象

类的实例、有函数属性的普通对象按引用交给别的语言，对方拿到代理：见 [插件 API：带方法的对象](plugin-api.md#带方法的对象)。

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

`load` 会检查宿主会检查的事：插件只用 `inject` 里声明的服务；不提供 `services` 里已有的名字（那是宿主提供的：`PluginTestError: wordcount: notes is already provided by the host`）；提供的服务带着 `provides` 里声明的每个方法（没有声明的服务，像 rutis 一样按对象上的方法、都按 async 调用；`t.methods(name)` 返回 rutis 看到的方法）；卸载时清理函数都运行了。默认是严格模式，值按跨进程的规则传递，见 [插件 API：严格模式](plugin-api.md#严格模式)；测试工具的其余接口见 [插件 API：TypeScript 与 Bun 的测试工具](plugin-api.md#typescript-与-bun-的测试工具)。

### 事件

`ctx.on(name, listener)` 监听事件，`ctx.emit(name, ...args)` 发出事件。监听器自动登记给宿主，什么都不用声明：发出方是同一个进程里的另一个插件、另一种语言的插件、宿主里的 Rust 代码还是相连的节点，结果都一样。其余发出方式（`parallel`、`serial`、`waterfall`、`bail`）、过滤与作用域见 [插件 API：事件](plugin-api.md#事件)。

测试要确认插件发出了某个事件，用 `t.on(name, listener, options)` 像宿主一样在根作用域监听：插件发出的、测试自己发出的都送到；严格模式下参数像从另一个进程收到的；选项与 `ctx.on` 相同（`prepend`、`global`）；返回撤销函数，不撤销时随这一行卸载撤掉。`host()` 装载的每一行都有它，共用一条总线。

<!-- example: typescript file=weather/test/asked.test.ts order=2 -->
```ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { definePlugin } from '@arcships/rutis'
import { load } from '@arcships/rutis/testing'

const asking = definePlugin({
  apply(ctx, config: { city: string }) {
    ctx.provide('weather', {
      async today() {
        ctx.emit('weather/asked', config.city)
        return 'sunny'
      },
    })
  },
})

test('each call emits weather/asked', async () => {
  const t = await load(asking, { config: { city: 'Oslo' } })
  const asked: string[] = []
  const off = t.on('weather/asked', (city: string) => { asked.push(city) })
  await t.service('weather').today()
  assert.deepEqual(asked, ['Oslo'])
  off()
  await t.unload()
})
```

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

### 调用一个方法

`dev` 启动后和每次重新加载后，打印现在有哪些服务、各由哪一行提供：`rutis-host dev: services: llm (llm), loader (host), weather (weather)`。

`dev` 运行时，另开一个终端，在项目目录里调用插件提供的服务，看它返回什么、出错时报什么，不用另写一个调用它的插件：

```console
$ npx rutis-host call weather.today
"sunny in Oslo"
```

每个参数是一个 JSON 值（字符串带引号：`'"Oslo"'`），结果打印成 JSON；错误写到标准错误并以 1 退出。见 [rutis-host：调用一个服务](rutis-host.md#调用一个服务)。

### 调试

`--debug` 打开 Node 运行时进程自己的检查器，监听 `127.0.0.1:9229`：

<!-- example: typescript run until="weather: running" order=100 -->
```bash
npx rutis-host dev --debug
```

输出里有 `Debugger listening on ws://127.0.0.1:9229/…`。用 Chrome 打开 `chrome://inspect`，在 Remote Target 下点 inspect；或者用 VS Code 的 attach 配置连端口 9229。在 Sources 里打开 `src/index.ts` 下断点，或者在代码里写 `debugger;`。

- 启动 `rutis-host` 的 npm 启动器不占这个端口。不要用 `NODE_OPTIONS=--inspect`：它也给启动器开检查器，先启动的启动器占掉端口，连上的是启动器；`--inspect-brk` 会让启动器停住。
- 重新加载后模块的地址带 `?rutis-reload=<n>`，是一个新的脚本：在新脚本里重新下断点，或者用 `debugger;`。
- 停在断点时整个 Node 运行时停着：这个运行时里其他行的调用也等它继续。
- 端口 9229 已被占用时，Node 打印 `Starting inspector on 127.0.0.1:9229 failed` 并照常运行，没有检查器：先结束占用端口的进程。
- `console.log` 的内容出现在 `dev` 的输出里。

## 5. 发布

```bash
npm publish --access public   # 先运行 prepack：tsc 生成 dist/
```

或者推一个 `v0.1.0` 这样的 tag，模板里的工作流会测试、`check` 并发布（仓库里需要 `NPM_TOKEN` secret）。`npm pack` 和 `npm publish` 都会先运行 `prepack`（`npm run build`），所以包里总有 `dist/`。

发布前给包起一个你自己的名字：`weather`、`greeter` 这样的短名字在 npm 上早被占用了。用带作用域的名字，改 `package.json` 的 `name`，例如 `"@your-name/weather"`（作用域是你的 npm 用户名或组织；首次发布带作用域的公开包要 `--access public`，模板的工作流已经带上）。宿主的行名就是这个包名。

依赖写 `"@arcships/rutis": "^0.18.0"` 这样的范围（`rutis-host new` 写的是它自己的版本）。插件用的是它写时那个次版本的 SDK：宿主里装着别的版本时，npm 给插件装它自己的一份，几个版本的 SDK 可以并存（插件靠 `Symbol.for` 的标记被认出，与是哪一份 SDK 无关）。插件上带有它所用的插件 API 版本（`definePlugin` 自动标记），宿主的运行时比它旧时会明确报错，而不是加载后出错。

## 6. 被宿主使用

宿主怎样装这个插件见 [rutis-host：宿主怎样装插件](rutis-host.md#宿主怎样装插件)。

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
