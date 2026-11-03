# 交来的函数怎样调用：默认异步，同步调用里同步

[English](design-function-refs-2026-11-03.en.md)

状态：草稿。日期：2026-11-03。基准：`main` `d2a8d069`（0.20.1）。
Issue：[#932](https://github.com/arcships/rutis/issues/932)（属于 [#663](https://github.com/arcships/rutis/issues/663) 的 O2、O5、A5）。按本稿实施：F-1～F-4（§七），都记在 #932 下。排期：0.21。
依据：[插件 API](guide/plugin-api.md)「值怎样传递」「同步调用与可重入」「Python 的测试工具」；[开放插件的宿主](guide/open-host.md) 第 5 节；[对象引用](design-object-refs-2026-10-27.md) §四（对象的方法都是异步调用）与 §六（等待环）；[诊断](design-diagnostics-2026-10-26.md) §4.3、§4.5；[#918](https://github.com/arcships/rutis/pull/918)（Node 的 Cordis 同步链）；[发布门](ci.md) 与 [安装场景](design-e2e-install-2026-10-11.md)。

范围外：

- 函数**交出去**的一侧：插件的函数被宿主、别的插件调用时的参数、返回值、错误、释放，都已经有规则（[插件 API](guide/plugin-api.md)「值怎样传递」），本稿不改。
- 协议：调用帧的 `sync` 标记、等待环检测（`bridge/sync-wait-cycle-refused`）都已存在，不加字段、不加能力、不升协议版本。
- 对象（带方法的对象、类的实例）的调用：已经是异步的（[对象引用](design-object-refs-2026-10-27.md) O2），本稿不改。
- Go、Swift、Kotlin、Rust 的写法：它们调用交来的函数本来就分开写（`Call(ctx, args, true)`、挂起 / 阻塞、`call` / `call_async`），本稿不改，也不改它们的行为。
- Python 测试工具里「同一个进程的值就是对象本身」（Cordis 的语义，[对象引用](design-object-refs-2026-10-27.md) §四）：本稿不改。测试里插件交来的函数是插件自己的函数，直接调用（§三 3.5），要让它也像跨进程一样，归 [#933](https://github.com/arcships/rutis/issues/933)。
- 调用的时限、取消：不加、不改（[诊断](design-diagnostics-2026-10-26.md) §二）。

## 一、现在怎样

### 1.1 Node、Bun：交来的函数同步调用，测试里的假服务不能写 `async`

[#663](https://github.com/arcships/rutis/issues/663) 的 0.19.0 评估（O2、O5、A5）里，宿主持有者和第三方插件作者都碰到：

> open-host.md 第 5 节给插件作者的 TypeScript 测试例子，原样运行就失败。例子里插件把处理函数登记给宿主（`commands.register(name, handler)`），测试用假的宿主服务调用这个处理函数；处理函数里再调用另一个假服务时报 `SyncWaitCycle`。

今天照指南原样做，`npm test` 里的测试失败，打印的是：

```text
SyncWaitCycle: await requires the Node thread occupied by its parent synchronous call; path: test:5 -> rutis-test:3 -> rutis-test:4
```

原因在运行时里，与例子无关：

- 收到的函数是 `#requestSync` 的代理（`node/rutis-runtime/src/session.mjs:473`）：调用它就是同步等结果，等的时候这个 Node 线程被占住（`#requestSync`，`session.mjs:573`）。
- 等的时候只跑属于这条调用链的帧，其他帧排在栈清空之后（`session.mjs:690`）。
- 处理函数里 `await notes.list()` 时，测试一侧那个假的 `notes.list` 返回的 Promise 需要这个线程推进一步才能落定，线程却在等；于是这次 `await` 被拒绝（`session.mjs:707`），也就是上面那句话。

Bun 的运行时的形状相同（`bun/rutis-bun/src/session.ts:468`、`:568`、`:694`）。

这条同步调用也决定了指南的写法：例子里的假服务必须直接返回值（`list: () => [...]`，不写 `async`），与真实服务的样子不一致。[#942](https://github.com/arcships/rutis/pull/942) 把例子改成直接返回结果，[#967](https://github.com/arcships/rutis/pull/967) 修掉了测试工具在等结果时被 `node --test` 取消（0.19.2），但**调用本身还是同步的**，假服务还是不能写 `async`：

```text
- 测试把收到的命令当函数调用，这种调用在测试的线程上同步等结果；等的时候这个线程无法完成 Promise。所以命令运行中要调用的假服务直接返回结果（`list: () => [...]`，不写 `async`）。写成 `async` 时调用失败：
```

（`docs/guide/open-host.md:798`，英文同。）

### 1.2 Python：交来的函数同步调用，阻塞事件循环

Python 收到的函数是 `RemoteFunction`（`python/rutis/rutis/peer.py:678`）。调用它走 `__call__` → `_call_sync`（`peer.py:686`、`:1438`）：在事件循环线程上阻塞，抽帧直到结果回来。对面交来的函数返回一个普通值时，`await f(...)` 拿到的是 Python 自己的错误，逐字是 `TypeError: object str can't be used in 'await' expression`（3.12；本机实测）：它返回的不是可 await 的对象。同一个类上有 `call_async`（`peer.py:690`），Python 指南里没有写它。

用 Python 写插件的人还有一层：插件收到的函数返回什么，取决于它是 `async def` 还是 `def`。`async def` 时 `f(...)` 拿到的是可 await 的结果，`def` 时是值本身，同一个写法在两种情况下不一样。

### 1.3 指南与发布门都没有守住它

这个例子带 `<!-- example: jot-command run -->`（`docs/guide/open-host.md:780`），但发布门只检查标记能否解析（`.github/workflows/install.yml:89`），示例本身只在发布后、用注册表上的包跑（`install.yml:263`–`270`，`tests/e2e/tests/doc_examples.rs:13`）。所以一个在发布门构建的包上根本跑不过的例子，可以一路进到发布后再被发现：0.19.0、0.19.1 发布后的检查都在这里停下（[#966](https://github.com/arcships/rutis/issues/966)），后面的指南没有跑到。

### 1.4 不能改的那一半

同步调用本身是 #918 定下的：Cordis 的 `emit`、`waterfall` 在 Node 里是同步的，同步只对发出方成立，发出方等的时候，分发调回发出方进程的调用是同步调用（`node/rutis-runtime/src/events.mjs:361`–`370`）。`sync` 方法、`SyncWaitCycle`、宿主转发同步调用前的等待环检查（`crates/rutis-bridge/src/session/rpc.rs:1869`）也都是写下来的行为。所以不能把「交来的函数」一律改成异步：在那些地方，代码本来就没法 `await`。

## 二、决策

| # | 决策 | 章节 |
| --- | --- | --- |
| F1 | **交来的函数默认异步**：与服务方法一样，Node、Bun 返回 Promise，Python 返回可 await 的对象；`await` 它，拿到的是对面那个函数的返回值 | 三 3.1、3.2 |
| F2 | **只在同步调用里同步**：这一次调用发生在一次同步调用里（我们自己正在同步等，或者我们正在履行对面的一次同步调用）时，调用同步返回结果，不返回 Promise。判据由运行时自己知道，开发者不用写第二遍 | 三 3.2、3.3 |
| F3 | **不加新名字**：Node、Bun 不给交来的函数加 `callSync` 之类；Python 的 `call_async` 与 `await f(...)` 是同一件事，写进指南 | 三 3.2 |
| F4 | **Cordis 的同步链不变**（#918）：运行时自己调用 `next` 的地方在同步链上改用内部的同步形式，插件作者的写法与今天相同 | 三 3.3 |
| F5 | **测试工具跟着真实宿主**：Node、Bun 的插件在另一个运行时进程里，测试调交来的函数要 `await`；Python 的插件在测试进程里，交来的是插件自己的函数（`def` 时直接用它，`async def` 时 `await` 它）。两处都写进指南 | 三 3.5 |
| F6 | **出错时**：真正的环照旧以 `SyncWaitCycle`（`name` 不变）失败，不卡死；文字由[诊断设计](design-diagnostics-2026-10-26.md)定，本稿在那里提一句（§四 4.3 的依赖） | 四 4.3 |
| F7 | **发布门跑一次只会装 npm / PyPI 的指南示例**：用发布门自己构建的包，在 Linux 上跑（含 open-host 第 5 节的两份）；发布后照旧用注册表上的包跑全部示例 | 六 |
| F8 | **线上格式、能力、Rust 一侧都不动**：`call` / `call_async`、协议的 `sync` 标记、等待环检测照旧 | 五 |

放弃的做法：

- **只改文档（教人写同步的测试）**：例子与真实宿主不一致，`async` 的假服务永远写不出来，第三方作者只能跳过这条测试（#663 评估原话）。放弃。
- **给交来的函数加一个显式的同步入口（`await f(...)` 与 `f.callSync(...)`）**：多一个概念，而且同步链上（`sync` 方法、Cordis 的同步分发）不写它就悄悄拿到 Promise。放弃。
- **一律异步，同步链改为不可能**：会破坏 `sync` 方法与 #918 的 Cordis 同步分发。放弃。
- **给 Python 的 `__call__` 之外再留一个同步的名字（`call_sync`）**：同一种调用两个名字，与应用其他地方的「`await` 服务方法」不一致，而且在同步调用里忘了用它只拿到一个协程。放弃。

## 三、规则

### 3.1 调用一个交来的函数 = 调用对面那个函数

- 参数、返回值、抛出的错误按「值怎样传递」跨边界（[插件 API](guide/plugin-api.md)），与今天相同。
- 同一个函数交来几次，拿到同一个代理；交回原来的进程，那边拿到的是原函数。
- 生命周期、释放与今天相同：接收方不再持有时代理释放，发出方放掉（[对象引用](design-object-refs-2026-10-27.md) §五给了同一套做法）。
- 不设时限，不能取消（JS 的 Promise 不能取消，与今天一样）。

### 3.2 什么时候异步，什么时候同步

**异步**：这一次调用不在同步调用里 —— 插件的 `apply`、`async` 服务方法、监听器、定时器、任何异步工作里。此时：

| 语言 | 交来的函数 `f` 怎么用 | 拿到什么 |
| --- | --- | --- |
| TypeScript、Bun | `await f(...)`，或把得到的 Promise 传下去 | Promise：返回对面的返回值；对面返回 Promise / 协程时，`await` 一路等到它的值 |
| Python | `await f(...)`（`f.call_async(...)` 是同一件事） | 可 await 的对象 |
| Go、Swift、Kotlin、Rust | 不变：`*advanced.Service` 的 `Call(ctx, args, false)`（同步形式是 `true`）、函数字段的结构体（阻塞）、`ctx.Call`、挂起方法、`call_async` | 各自的异步形式 |

**同步**：这一次调用发生在一个同步调用里。两个来源，运行时都自己知道：

1. **我们自己正在同步等**：我们发出了一次同步调用（`sync` 方法、`bail`、Cordis 的同步 `emit` / `waterfall` 发出的分发），在我们等它的这段时间里，这个线程 / 事件循环被占着。
2. **我们正在履行对面的一次同步调用**：对面用 `sync` 调用了我们提供的 `sync` 方法（或我们这边的监听器被同步调用），我们正在它的调用里。

此时对交来的函数的调用同步发出，直接得到结果（Python 就是不加 `await` 的 `f(...)`）—— 因为那里没法 `await`：JS 的线程被占着，微任务不会跑；Python 的事件循环被占着，协程不会跑。这条与 #918 定下的同步语义是同一条：**同步等待只用在本来就同步的调用里。**

判断的实现位置（各运行时自己的状态，不外露）：

| 运行时 | 标记 |
| --- | --- |
| Node（`session.mjs`）、Bun（`session.ts`） | 一个计数器：`#requestSync` 等待期间加一，正在执行的入站帧带 `sync` 时加一；交来的函数的代理看它决定走 `#requestSync` 还是 `#requestAsync` |
| Python（`peer.py`） | 一个计数器：`_call_sync` 等待期间加一，`_execute` 执行带 `sync` 的帧时加一（协程方法在第一个挂起之后不再算，那时事件循环已经能跑）；`RemoteFunction.__call__` 看它决定返回可 await 的对象还是值 |

- **一种写法、两种返回**：JS 与 Python 里调用交来的函数是同一个表达式，返回值是不是 Promise 由上下文决定。这与宿主生成的类型一致：函数参数的返回类型本来就写 `string | Promise<string>`（`rutis-host types` 写出，[开放插件的宿主](guide/open-host.md) 第 2 节）。指南教的写法是 `await`（Python 里 `await f(...)`，与 `f.call_async(...)` 相同）；同步调用里去掉 `await`，写裸的 `f(...)`（那里写 `await` 也跑不动）。
- **为什么不在两个运行时里选同一种做法**：JS 与 Python 的调用表达式不区分同步 / 异步，`sync` 方法又要能调用交来的函数，所以只能由运行时按上下文决定；Go、Swift、Kotlin、Rust 的调用本来就分开写，不需要这条。

### 3.3 同步调用里不能 `await`

同步调用里 `await` 交来的函数的结果，就是今天的 `SyncWaitCycle`：

- 运行时自己发现时（结果要等这个线程 / 循环推进才能落定）按 §四 4.3 第 1 行说，只让这一次调用失败，不卡死；
- 宿主转发同步调用前发现等待成环时按 §四 4.3 第 2 行说，这次同步调用不转发。

两者都是「做不到的事明确报错」（[设计哲学](design-philosophy.md) §七），行为与今天相同，只是走过的新写法变少。

### 3.4 对面调回来（可重入）

- **异步调用时**：我们的线程 / 事件循环是空的，对面随时可以调用我们（服务方法、事件、交回给我们的函数），与普通服务调用一样。对面在回答我们的调用之前调用我们，也行。
- **同步调用时**：对面调回我们的调用照样运行，属于这条链的帧先跑（Node、Bun；`session.mjs:690`），Python 在同步等待期间也执行其他进来的调用（`peer.py:1688`，「可重入」）。这时对面调回我们的**同步**调用，如果它自己又在等我们，宿主按等待环拒绝（§四 4.3 第 2 行）。
- **不要在同一次同步调用里等一个只有对面回答才落定的 Promise**：那是真的环，见上。要求对面尽快做、或者改用 `async` 方法。

### 3.5 测试工具

| 语言 | 测试里交来的函数 | 怎么写 |
| --- | --- | --- |
| TypeScript、Bun | 插件的行在另一个运行时进程里（`rutis-host test` 起的宿主 + 测试自己的运行时），交来的是跨进程的函数 | 与真实宿主一样：`await`；假服务可以写 `async` |
| Python | 插件的行在测试自己的进程里（[Python 的测试工具](guide/plugin-api.md#python-的测试工具)），交来的是插件自己的函数 | `async def` 的 `await` 它，普通 `def` 的直接用它的值；假服务可以写 `async def`。跨进程（另一个插件、宿主给的回调）交来的是 `RemoteFunction`：用异步写法 `await f(...)`（与 `f.call_async(...)` 相同），裸的 `f(...)` 只留给同步调用（§三 3.2、3.3） |

两处都不改测试工具的语义；Python 那条差别写进 Python 指南与插件 API 指南，与本稿 §四 4.2 一致。

### 3.6 Cordis（Node）

- Cordis 的同步 `emit` / `waterfall` 照 #918：发出方同步等，`next` 在同步链上返回链上的值。
- 运行时内部调用 `next` 的代码在同步链上改用内部的同步形式（`Session.callFunctionSync`，与已有的 `callFunctionAsync` 对称），插件作者的写法不变（`events.mjs:361`–`370`）。`next` 是交来的函数，它照 3.2 的规则：同步链上同步返回，别处返回 Promise。
- 插件里的 Cordis 代码调用别的进程交来的函数时，按 3.2 判断。

### 3.7 出错时的文字：底层错误照原样

- **消息的主体是底层错误自己的话，逐字**：serde 的 ``invalid type: integer `42`, expected a string``、Python 的 `KeyError: 'town'`、Go 的 `json: unsupported type: chan int`、插件自己 `throw` 的错误。运行时与宿主原样带过去，不改写、不翻译、不概括。
- **rutis 只加一句短前缀**，回答「这是我哪一次调用引起的」（哪个服务、哪个方法、第几个参数、哪一行）。
- **不重复底层错误已经说过的话**，也不替它解释。前缀之外的部分就是它自己的文字。
- **只有 rutis 自己造的错误才改写成开发者看得懂的话**：它自己的行话、内部编号（`SyncWaitCycle` 那句里的 `path: test:5 -> …`）、不该露出的实现词。那不是底层错误，改写它是这条规则要的，不是例外。
- 拿不准时保留原文。

## 四、开发者看到什么

### 4.1 插件作者：TypeScript（正常）

指南第 5 节的例子（`docs/guide/open-host.md` 第 5 节）：模板、生成的类型不变，与今天相同。测试里的假服务回到 `async`，与真实服务一样 —— 这也是这个例子今天唯一改的一行：

```ts
// count/test/index.test.ts
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

```console
$ npm test
✔ counts the notes and their words (127.752458ms)
ℹ tests 1
ℹ suites 0
ℹ pass 1
ℹ fail 0
ℹ cancelled 0
ℹ skipped 0
ℹ todo 0
ℹ duration_ms 161.385083
```

`dev` 里照旧：

```console
$ npx rutis-host dev
count: running
$ npx rutis-host call commands.run '"count"'
"2 notes, 5 words"
```

指南第 5 节里「假服务要直接返回结果、写成 `async` 会失败」那一段和它的 `SyncWaitCycle` 输出整段删掉，换成一句：测试调用插件交来的函数要 `await`，假服务可以写 `async`。

插件在别处收到的函数同样写法：

```ts
// 另一个插件交来的函数，或宿主给的回调：await 它
const answer = await answerFor('what is 6 times 7?')
```

### 4.2 插件作者：Python（正常，与 TypeScript 的一处不同）

同一节给 Python 作者的第二份例子（新增，标记 `jot-command-python`，发布门里跑）：

```python
# count/src/count/__init__.py
from rutis import define_plugin


async def apply(ctx, config):
    commands = ctx.use("commands")
    notes = ctx.use("notes")

    async def count(args):
        all_notes = await notes.list()
        words = len(" ".join(all_notes).split())
        return f"{len(all_notes)} notes, {words} words"

    await commands.register("count", count)


plugin = define_plugin(apply, inject=["notes", "commands"])
```

`dev` 用的假 jot（`py:fake_jot` 这一行；与 TypeScript 那份假 jot 一样，只是写在 Python 里）：

```python
# count/dev/fake_jot.py
from rutis import define_plugin


class Jot:
    def __init__(self):
        self.notes = ["buy milk", "call Ada back"]
        self.commands = {}

    async def list(self):
        return list(self.notes)

    async def add(self, text):
        self.notes.append(text)

    async def register(self, name, run):
        self.commands[name] = run

    async def run(self, name, args=None):
        return await self.commands[name](args or [])


def apply(ctx, config):
    jot = Jot()
    ctx.provide("notes", jot)
    ctx.provide("commands", jot)


plugin = define_plugin(
    apply,
    provides={"notes": {"add": "async", "list": "async"}, "commands": {"register": "async", "run": "async"}},
)
```

`count/rutis.dev.json`：

```json
{
  "format": 1,
  "rows": [
    { "id": "count" },
    { "id": "jot", "name": "py:fake_jot" }
  ]
}
```

```python
# count/tests/test_plugin.py：插件在测试自己的进程里，交来的是插件自己的函数
import asyncio
import unittest

from rutis.testing import load

from count import plugin


class FakeNotes:
    async def list(self):
        return ["buy milk", "call Ada back"]


class FakeCommands:
    def __init__(self):
        self.runs = {}

    async def register(self, name, run):
        self.runs[name] = run


class Plugin(unittest.TestCase):
    def test_counts_the_notes_and_their_words(self):
        async def go():
            commands = FakeCommands()
            async with load(plugin, services={"notes": FakeNotes(), "commands": commands}) as t:
                self.assertEqual(await commands.runs["count"]([]), "2 notes, 5 words")

        asyncio.run(go())


if __name__ == "__main__":
    unittest.main()
```

```console
$ uv run python -m unittest discover -s tests
.
----------------------------------------------------------------------
Ran 1 test in 0.412s

OK
```

与 TypeScript 例子只有一处不同，写进指南：**测试里拿到的函数是插件自己的函数**。它是 `async def` 时 `await` 它（上面的例子）；插件交来的是普通 `def`（返回 `str`）时直接用它；`await` 一个 `str` 时 Python 抛出它自己的 `TypeError: object str can't be used in 'await' expression`，与 rutis 无关。**跨进程收到的函数用异步写法**：另一个插件、宿主给的回调是 `RemoteFunction`，`await f(...)`（与 `f.call_async(...)` 相同）拿到返回值；裸的 `f(...)` 同步返回结果、占着事件循环，只在同步调用里用（`sync` 方法、Cordis 的同步分发，§三 3.3），那里也没法 `await`。今天（0.20）跨进程的 `f(...)` 一律同步返回，所以 `await f(...)` 在普通值上报上面那个 `TypeError`；本稿去掉的正是这一条（§三 3.2）。`dev` 里用 `py:fake_jot` 这一行时也是同一个进程，`npx rutis-host call commands.run '"count"'` 与 TypeScript 例子相同：

```console
$ npx rutis-host call commands.run '"count"'
"2 notes, 5 words"
```

### 4.3 出错时

这份设计不写错误码与给人看的措辞：它们是[诊断设计](design-diagnostics-2026-10-26.md) §4.3、§4.5 的事。这里只写这两种失败怎样出现、记在哪一行、怎么改，措辞按那份设计稿。

| 情况 | 怎样触发 | 记在哪一行 | 怎样改 |
| --- | --- | --- | --- |
| 同步调用里 `await` 交来的函数的结果 | `sync` 方法（或 Cordis 的同步分发）里 `await f()`，`f` 的结果要等这个线程 / 循环推进才能落定 | 调用方这一行；在测试里是这一条测试失败 | 同步调用里去不掉 `await`：交来的函数在那里同步返回结果，直接用它；或者把这个方法声明成 `async` |
| 两个运行时经交来的函数互相同步调用 | A 在同步调用里调用 B 交来的函数，B 在它的同步调用里又调用 A 的 | 发出这次同步调用的行 | 改用 `async` 方法，或换个时机（[插件 API](guide/plugin-api.md)「同步调用与可重入」）；不要立刻重试同一个调用 |

- 两种失败都以 `SyncWaitCycle` 结束，`name` 与今天相同；它的码与句子按[诊断设计](design-diagnostics-2026-10-26.md)。今天 Node、Bun 那条带内部的调用编号（原样见 §一 1.1），宿主那条带走过哪些行。
- **依赖**：本稿提议诊断设计在 §4.3 加一行、在 §4.5 的措辞里写成一句运行时自己发现时的说法 —— 本稿提议的那句是 `SyncWaitCycle: <what> would wait on itself: it is awaited inside the synchronous call that produces it (synchronous calls)`（`<what>` 是这次调用：`a function`、`the method get`、`weather.tomorrow`）。采纳与措辞由那份设计稿的负责人决定。改这一句正是 §三 3.7 的最后一条：`SyncWaitCycle` 是 rutis 自己造的错，今天那句里带 rutis 的内部编号；插件抛出的错误、Python 的 `TypeError`、Go 编码器的错误都是底层错误，照原样，本稿不改它们的文字。
- 其余错误照旧（`bad-value`、`no-method`、`peer-gone`，见那份设计稿 §4.3）。
- 正常时（§4.1、§4.2）没有任何新的输出：调用照常返回，测试照常通过。

### 4.4 宿主持有者

- 宿主调用插件交来的函数，写法不变：Rust 是 `call`（同步）与 `call_async`（异步），Go、Swift、Kotlin 各自的写法不变。
- 宿主把回调交给插件时，插件的代码 `await` 它；Node、Bun、Python 的插件在 `sync` 方法里照旧同步拿到结果。宿主一侧不改代码。
- `rutis-host status` 里「看不到按引用传过来的函数的调用」（[开放插件的宿主](guide/open-host.md) 第 6 节）不变：它们是本机运行时与本机运行时之间的调用。
- `SyncWaitCycle` 经会话的样子不变（`crates/rutis-bridge/src/session/protocol.rs:89`–`111`、`:140`–`144`）：
  - 宿主自己拒绝时是 `session::Error::SyncWaitCycle(message)`（`crates/rutis-bridge/src/session/rpc.rs:1875`）；它会以 `Failure { name: "SyncWaitCycle", message, graph: None }` 落到运行时，**没有 `code`、没有 `data`、没有图**；
  - 运行时抛出的那个带错误图，宿主收到的是 `session::Error::Remote { name: "SyncWaitCycle", message, graph }`（`crates/rutis-bridge/tests/rpc_callbacks.rs:221`、`:480`）；不带图时又收回成 `Error::SyncWaitCycle`；
  - 运行时的代码看到的是：Node、Bun 拿到一个 `name` 为 `SyncWaitCycle` 的 `Error`，没有 `code`（`node/rutis-runtime/src/errors.mjs:133`–`140`）；Python 拿到 `rutis.advanced.SyncWaitCycle`（`python/rutis/rutis/peer.py:416`）；Go 拿到 `*rutis.RemoteError`，`errors.Is(err, advanced.ErrSyncWaitCycle)` 成立（`advanced` 即 `rutis/advanced`；`go/rutis/internal/errs/errs.go:38`）。
- 线上不带新的字段：`code`、`data` 只在图里（那份设计稿 §4.3），`SyncWaitCycle` 没有图。

### 4.5 用 `rutis-host` 组装插件的人

- `rutis-host new --lang node|bun|python` 生成的模板、`rutis.dev.json`、`dev`、`call` 都不变。
- 他们写的假服务（`dev` 里的行）可以写成 `async`：`dev` 与 `call` 的输出与今天相同（§4.1）。

### 4.6 部署的人

看不到变化：进程数、用的包、配置、停止与退出码都不变。

### 4.7 要懂的概念

- **插件作者**（预算 7，见 [#663](https://github.com/arcships/rutis/issues/663)）：0 个新概念。用的是已有的「异步方法 / `async`」与「`sync` 方法」；交来的函数按它们判断，指南写一句话。
- **插件作者（Python 的测试）**：多知道一件事（测试里拿到的是插件自己的函数；跨进程交来的写异步的 `await f(...)`），不是新名字，写在同一节里。
- **宿主持有者**（预算 10）：0 个新概念。
- **用 `rutis-host` 组装插件的人**（预算 6）、**部署的人**（预算 5）：0 个。

## 五、兼容与迁移

### 5.1 协议与版本，逐项

| 项 | 现在 | 本稿 |
| --- | --- | --- |
| 会话协议 | 4，六种语言的运行时与宿主各有一处常量（5.1a） | 不变：不加帧、不加字段、不加能力（`sync` 标记与 `sync-stack` / `reentrant-sync` 都照旧） |
| `rutis.json` | `FORMAT` = 1（`crates/rutis-host/src/config.rs:78`） | 不变：没有新的行字段，也没有新的 `runtimes` 写法 |
| 插件 API | 3，六种语言的 SDK 声明它，Rust 没有（5.1b） | 不变：`definePlugin` / `define_plugin` 的名字与形状没有增减，交来的函数的调用方式不在 API 版本里（它是运行时的行为） |
| 线上错误 | `Failure { name, message, graph? }`（`crates/rutis-bridge/src/session/protocol.rs:89`） | 不变：`SyncWaitCycle` 仍是 `name`，没有 `code`、没有 `data`、没有图（§四 4.4） |
| 生成的类型 | `rutis-host types` 写出 `.d.ts` 与 `.py`（`run: (arg0: string[]) => string \| Promise<string>`） | 不变，不用重新生成：回调的返回类型本来就把两种都写上，`await` 在异步与同步两种上下文都通过类型检查（§三 3.2） |
| Rust 的 `call` / `call_async`、Go / Swift / Kotlin 的写法、Python 的 `call_async` | — | 不变 |

#### 5.1a 会话协议 4：每种语言的那一处

| 语言 | 常量 | 值 |
| --- | --- | --- |
| Node | `ENDPOINT_PROTOCOL`，`node/rutis-runtime/src/session.mjs:58` | 4 |
| Bun | `ENDPOINT_PROTOCOL`，`bun/rutis-bun/src/session.ts:57` | 4 |
| Python | `ENDPOINT_PROTOCOL`，`python/rutis/rutis/peer.py:42` | 4 |
| Go | `EndpointProtocol`，`go/rutis/internal/peer/peer.go:34` | 4 |
| Kotlin | `ENDPOINT_PROTOCOL`，`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/peer/Peer.kt:42` | 4 |
| Swift | `endpointProtocol`，`swift/rutis/Sources/Rutis/Session/Peer.swift:13` | 4 |
| Rust（宿主） | `ENDPOINT_PROTOCOL`，`crates/rutis-bridge/src/session/mod.rs:14` | 4 |

握手时对方不是这个值就被拒绝，并说明升级哪一端：Node 的那句以 `incompatible session: the far end speaks protocol <对方>, this side <本地>` 开头（`node/rutis-runtime/src/session.mjs:86`），Python（`python/rutis/rutis/peer.py:50`）与 Rust（`crates/rutis-bridge/src/session/rpc.rs:1229`）同形，六种语言的运行时与宿主各有一处。本稿不碰。

#### 5.1b 插件 API 3：谁声明它

| 语言 | 常量 | 值 |
| --- | --- | --- |
| Node | `PLUGIN_API`，`node/rutis/src/advanced.mjs:15` | 3 |
| Bun | `PLUGIN_API`，`bun/rutis-bun/src/plugin.ts:21` | 3 |
| Python | `PLUGIN_API`，`python/rutis/rutis/plugin.py:125` | 3 |
| Go | `PluginAPI`，`go/rutis/internal/runtime/version.go:9`（清单里的 `pluginApi`，`go/rutis/internal/runtime/serve.go:133`） | 3 |
| Kotlin | `PLUGIN_API`，`kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/advanced/Versions.kt:7` | 3 |
| Swift | `Advanced.pluginAPI`，`swift/rutis/Sources/Rutis/Version.swift:28` | 3 |
| Rust | **没有这个常量**：Rust 插件编进宿主，没有清单、没有 `pluginApi`；宿主只读二进制运行时的清单，那里的常量是 `crates/rutis-loader/src/runtime/go.rs:34` 与 `swift.rs:33`，比自己的大就拒绝（`crates/rutis-loader/src/runtime/binary.rs:1111`） | — |

插件声明得比运行时新时，运行时拒绝，说出插件要的版本与运行时的版本（Node 的那句还提示升哪个包）：Node `node/rutis-runtime/src/leaf.mjs:25`、Bun `bun/rutis-bun/src/plugin.ts:100`、Python `python/rutis/rutis/runner.py:1417`、Go `go/rutis/internal/runtime/runtime.go:280`、Kotlin `kotlin/rutis/rutis/src/shared/kotlin/arcships/rutis/internal/Runtime.kt:267`、Swift `swift/rutis/Sources/Rutis/API/Runtime.swift:420`。本稿不改版本号，也不改这条判断。

### 5.2 混用：旧的一端遇到新的一端

差别在本端的运行时里，不跨进程约定，所以任意组合都照常工作：

| 组合 | 结果 |
| --- | --- |
| 0.21 的宿主 + 0.20 的运行时 | 宿主不变；那个运行时里的交来的函数照旧同步调用（0.20 的行为）。同一个插件照 0.21 写成 `await`，在 0.20 的 Node、Bun 上也成立（`await` 一个不是 Promise 的值原样给出它）；Python 上 0.20 的 `f(...)` 同步返回值，`await` 一个普通值会失败，所以同一份 Python 插件不要在两种运行时上混用 |
| 0.20 的宿主 + 0.21 的运行时 | 宿主不变；运行时按本稿（异步默认、同步调用里同步）。同步调用的等待环检查、`SyncWaitCycle` 都是宿主已有的，行为与今天相同 |
| 0.21 的调用方调用 0.20 的运行时里的函数 | 调用方发的是异步调用（不在同步调用里时）：0.20 的运行时照常回答，`await` 得到结果；在同步调用里调用时照旧同步，与今天相同 |
| 0.20 的调用方调用 0.21 的运行时里的函数 | 0.20 的调用方发同步调用（它的规则）；0.21 的运行时在同步调用里同步返回结果，与今天相同 |

### 5.3 写进《迁移到 0.21》（两份，新建）

- **交来的函数默认异步（Node、Bun、Python）**：在 `apply`、`async` 方法、监听器、定时器里调用交来的函数，现在得到 Promise / 可 await 的对象。过去的写法（同步等结果）在同步调用里仍然成立；不在同步调用里等它、又把它当值的代码要改成 `await`：
  - TypeScript、Bun：`const text = f()` → `const text = await f()`（`await` 一个不是 Promise 的值原样给出它，所以改过的写法在 0.20 上也能运行）；
  - Python：`text = f()` → `text = await f()`（`f.call_async(...)` 同一件事）；同步调用里保持 `f()`，它在那里同步返回结果、也没有别处可写。
- **测试里的假服务可以写 `async`**：过去必须直接返回结果的地方现在两种都行。写成 `async` 而调用的地方又 `await` 的测试，在 0.21 上通过、在 0.20 上失败——按 0.21 的写法改，不要交叉使用。
- **`SyncWaitCycle` 的文字**（Node、Bun）：按诊断设计采纳的说法（§四 4.3 的依赖）；`name` 与按它判断的代码不变。
- **不兼容的只有一处**：上面第一条（交来的函数在异步上下文里改成异步）。其余都是行为不变或变宽松。

### 5.4 保证表

[guarantees](guarantees.md) 新增 `bridge/function-refs`：交来的函数在异步上下文里是异步、在同步调用里同步、真正的环以 `SyncWaitCycle` 失败；`boundary/5.6` 那一条加一句指向本稿。

## 六、发布门跑一次指南示例

今天（`.github/workflows/install.yml`）：`plan` 只跑 `node tools/doc-examples.mjs --check`（第 89 行），标记过的示例在 `install.yml:263`–`270` 只在 `RUTIS_E2E_SOURCE == 'registry'`（发布之后）跑。发布门构建的包（`RUTIS_E2E_ARTIFACTS`）已经有，`tests/e2e/tests/devx.rs` 就是用它们在没有发布的版本上跑开发者场景；示例没有用上它们。

决策（F7）：

1. **新增本地包源**：`tools/install/artifact-registry.mjs` 把 `RUTIS_E2E_ARTIFACTS/npm` 里的 tarball 当成本地 npm registry：包名 → packument（一个版本，`dist.tarball` 指向它自己），tarball 原样发出。示例里的 `npx @arcships/rutis-host new …`、`npm install`、`npx rutis-host …` 都照写，靠环境变量 `npm_config_registry` 指过来。Python 用已有的做法：`UV_FIND_LINKS` 指 `RUTIS_E2E_ARTIFACTS/wheels`，索引指一个空目录，`UV_INDEX_URL` / `PIP_NO_INDEX` 同 `tests/e2e/tests/plugin_install.rs:385`。
2. **示例照原样跑**：`Scenario::installed`（`tests/e2e/src/lib.rs:265`）加一个用本构建的包的变体：新的 home、npm 与 uv 的缓存在里面，npm / uv 的环境指向第 1 条，其余步骤与发布后的那次完全相同（例子里的命令一行不改）。
3. **哪些示例在发布门里跑**：只装 npm、PyPI 包的示例——`jot-command`、`jot-command-python`（两份指南各自的 `-en` 名字）、`plugin-api-ts`、`plugin-api-py`、`typescript`、`bun`、`python`、`rutis-host`，在 Linux 上跑；需要 crates.io 或 Go 模块代理的（`jot`、`rust`、`go`、`swift`）仍在发布后用注册表上的包跑，因为发布门里没有这些包。
4. **发布后照旧**：`install.yml:263`–`270` 的那一步不变，跑全部示例。

这样 open-host 第 5 节的例子在每次发布门里都跑一次，用的是这次要发的包；发布后的那次再确认注册表上的包也一样。

## 七、实施

| 阶段 | 内容 |
| --- | --- |
| F-1 | Node、Bun 运行时：交来的函数的代理按上下文走 `#requestSync` / `#requestAsync`；同步调用标记（§三 3.2 的表）；`events.mjs` 的同步 `next` 改用内部的 `callFunctionSync`；`SyncWaitCycle` 的文字按诊断设计采纳的那一句（§四 4.3）；运行时的测试 |
| F-2 | Python 运行时：`RemoteFunction.__call__` 按上下文返回可 await 的对象或值，`call_async` 与之相同；`_execute` 执行带 `sync` 的帧时算同步调用；运行时的测试 |
| F-3 | 文档（两份）：open-host 第 5 节（删掉 `SyncWaitCycle` 那一段、假服务回到 `async`、新增 Python 例子与它的那处不同）；[插件 API](guide/plugin-api.md)（「值怎样传递」函数一行、「同步调用与可重入」、Python 与 TypeScript 的测试工具段）；[TypeScript](guide/typescript-plugin.md)、[Bun](guide/bun-plugin.md)、[Python](guide/python-plugin.md) 指南的测试一节；《迁移到 0.21》；保证表 |
| F-4 | 发布门：`tools/install/artifact-registry.mjs`、`Scenario` 的本地包变体、`install.yml` 在 `source=local` 时跑第 3 条列出的示例；`doc_examples.rs` 的本地模式 |

F-1 与 F-2 可以并行；F-3 的两份文档在 F-1、F-2 之后一起改；F-4 与前三者独立。

完成条件（#932 在全部满足后关闭）：

- [ ] open-host 第 5 节的 TypeScript 例子原样 `npm test` 通过，假服务写 `async`；Python 例子同样通过（两种语言各一条）。
- [ ] Node、Bun、Python 各有测试：交来的函数在异步上下文里返回 Promise / 可 await 的对象；在同步调用里同步返回结果；真正的环以 `SyncWaitCycle` 失败、不卡死。
- [ ] 诊断设计采纳 §四 4.3 提议的那一句（依赖）；Node、Bun、Python 的文字与它一致，验收测试按那份设计稿的文字比较。
- [ ] 两份文档同步改完，第 5 节的例子在发布门里跑过。
- [ ] 发布门在 `source=local` 时用本构建的包跑第 3 条列出的示例并通过；发布后那次照旧全部通过。
- [ ] 保证表新增 `bridge/function-refs`，指名的测试通过（`node tools/check-guarantees.mjs`）。
