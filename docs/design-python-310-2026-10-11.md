# Python 3.10 支持：自己实现 eager start（设计稿）

[English](design-python-310-2026-10-11.en.md)

状态：设计，待评审。日期：2026-10-11。基准：`main` `0941ef4`。
依据：#196（含 2026-10-09 的决定和 #218 遗留的 `accept()` 问题）、#204（CI 范围调整）、#197 / #218（websockets 下限 15）、#193（PyPI 安装冒烟）、#228（策略 B）。属于 #183 第一步。
规范：[质量规范](quality-standard.md) Q10.2、Q10.4、Q6.7.1、Q5.2.2；[现状](quality-status.md) 风险 MX。

范围外：Python 3.9 及更早；Node 最低版本（#195）；websockets 下限本身（#197 已合入）；CI 里其他版本的改动（由 #204 的 #256 做）。

## 一、决策

| # | 决策 |
| --- | --- |
| D1 | 最低 Python 版本 3.10；`requires-python = ">=3.10"`（三处）。3.9 不支持 |
| D2 | 3.12 及以上继续用原生 `asyncio.Task(..., eager_start=True)` |
| D3 | 3.10、3.11 用新模块 `rutis/_eager.py` 模拟 eager start，行为与原生一致（第三节），差异写明并有界（3.9 节） |
| D4 | 两条路径都直接构造 `asyncio.Task`，不经过 `loop.create_task`，因此都不使用 `loop.set_task_factory` 设置的工厂（与现在 3.12 路径一致） |
| D5 | 两条路径共用一组带固定预期的测试，在 3.10、3.11、3.12+ 上都断言同一结果 |
| D6 | `websocket.Listener` 的监听失败后，`accept()` 立即抛出；`python -m rutis` 因此以非 0 退出码结束并打印原因 |
| D7 | CI：Linux 上的任务用 3.10，macOS 上的任务用最新版；不新增 PR 任务（与 #204 一致） |

## 二、现状

### 2.1 依赖 3.12 的地方

`python/rutis/rutis/peer.py` `Peer._task`（第 911–922 行）：

```python
context = contextvars.copy_context()
if sys.version_info >= (3, 12):
    task = asyncio.Task(coroutine, loop=self.loop, context=context, eager_start=True)
else:
    task = self.loop.create_task(coroutine, context=context)
```

`_task` 有两个调用点：

- `_execute`（第 890 行）：进来的 `invoke` / `call` 返回了协程（`async def` 方法），在 `_execute` 还没回复之前把它变成任务，然后 `_respond` 把任务作为 `future` 引用回复给对方；
- `_encode`（第 498 行）：一个协程作为值被编码（例如放在返回值里）。

两处都在 loop 线程上、loop 运行中调用。`_execute` 调用 `_task` 时，`self._context`（调用链 `path`）已设为这次调用的链，`self._sync_path` 也一样；`copy_context()` 把链带进任务，之后的步骤发出的调用 `path` 里含这次调用的号。

### 2.2 为什么需要 eager start

`docs/plan/analysis/multilang/python.en.md` §3.3 第 2 条：Python 协程在被调度前不执行，JS 的 async 函数立即执行同步前半段。eager start 让 `async def` 方法和 JS 一样：

1. 前半段在回复之前执行完；
2. 从不挂起的协程，任务在创建时就已完成。

第 2 条决定 `SyncWaitCycle` 会不会误报。`_execute` 处理 `await` 帧时（第 864–876 行）：被等待的 future 已完成就直接回复结果；还没完成、且这个 `await` 属于 Python 正在同步等待的链，就回复 `SyncWaitCycle`，因为 loop 被同步调用占着，future 不可能在等待期间完成。没有 eager start 时，一个从不挂起的 `async def` 方法返回的任务要等下一轮 loop 才执行，在同步链上被 `await` 时就是"未完成"，得到 `SyncWaitCycle`；有 eager start 时它已经完成，正常返回。单元测试 `test_a_finished_future_is_ready_during_a_synchronous_call` 和 `test_a_pending_future_on_the_chain_is_a_cycle` 分别覆盖这两种分支，但都是用手工构造的 future，没有经过 `_task`。

### 2.3 在 3.10 上实测（本机，未改仓库）

- `python/rutis` 单元测试：3.10.16（装 websockets 15.0.1）41 个全部通过；3.11.11（不装 websockets）通过，跳过 9 个。
- Rust 侧：`cargo test -p rutis-loader --features node,python --test python_rows --test multilang --test instance_runtimes`，`python3` 指向 3.10.16，8 个测试里 7 个失败（通过的那个不启动 Python），错误是 `BaseEventLoop.create_task() got an unexpected keyword argument 'context'`，以及 `coroutine 'Runtime.load' was never awaited`。与 #196 调研一致：Python 单元测试没有经过 `_task`。
- 用第三节的原型（`_eager.py`）替换 3.12 以下的分支后，同样的测试在 3.10.16 上全部通过；loader（`node,python,peer`）和 bridge（全部 feature）的全部测试也通过，见第十二节。

## 三、eager start 的模拟

### 3.1 原生 3.12 的行为（我们依赖的部分）

`asyncio.Task(coro, loop=loop, context=ctx, eager_start=True)`，loop 运行中时：

1. 把当前任务换成新任务，在 `ctx` 里同步执行协程的第一步，再换回原来的当前任务；
2. 第一步结束（返回或抛出）：任务已完成，不再调度；
3. 第一步挂起在 future `F` 上：立即在 `F` 上登记唤醒，之后与普通任务相同；挂起在 `None` 上（`await asyncio.sleep(0)`）：下一轮 loop 继续；
4. 之后每一步都在 `ctx` 里执行；
5. 不经过任务工厂。

### 3.2 算法

新模块 `python/rutis/rutis/_eager.py`，只在 3.10、3.11 上被 `_task` 使用（测试在所有版本上都用它，见 6.1）：

```python
def start(loop, coro, context):
    """asyncio.Task(coro, loop=loop, context=context, eager_start=True)
    for Python 3.10 and 3.11."""
    rest = _Rest(coro, context)
    task = asyncio.Task(rest, loop=loop)   # 调度第一次 __step；不经过工厂
    previous = _tasks._current_tasks.get(loop)
    _tasks._current_tasks[loop] = task                # 第一步期间 current_task() 是 task
    try:
        try:
            rest.waited = context.run(coro.send, None) # 第一步
            return task                                # 挂起了：剩余部分由 task 驱动
        except StopIteration as stop:
            rest.outcome = (True, stop.value)
        except (KeyboardInterrupt, SystemExit) as error:
            rest.outcome = (False, error)
            raise
        except BaseException as error:
            rest.outcome = (False, error)
    finally:
        if previous is None:
            _tasks._current_tasks.pop(loop, None)
        else:
            _tasks._current_tasks[loop] = previous
    # 第一步就结束了：返回一个已完成的 future。task 在它的第一次 __step
    # 以同样的结果结束，异常由 done 回调取走，不重复记日志。
    task.add_done_callback(_retrieve)
    return _completed(loop, rest.outcome)
```

`_Rest` 是实现了协程协议（`send`、`throw`、`close`，注册为 `collections.abc.Coroutine`）的对象，由 `task` 驱动，代表"第一步之后的剩余部分"：

- 第一次 `send`：
  - 第一步已结束：按 `outcome` 抛 `StopIteration(value)` 或那个异常，`task` 以同样结果结束；
  - 第一步挂起在 `None` 上：已经过了一轮 loop，直接 `context.run(coro.send, None)` 继续；
  - 第一步挂起在 future `F` 上且 `F` 已完成：把 `F._asyncio_future_blocking` 置回 `False`，直接 `context.run(coro.send, None)` 继续（`Future.__await__` 返回 `F.result()`，`F` 被取消时在 `await` 处抛 `CancelledError`）；
  - 其余情况：把 `F` 交给 `task`（作为 `send` 的返回值），由 `task` 登记唤醒，并照常检查 `F` 是否属于同一个 loop、是不是合法的 yield。
- 之后每次 `send(value)`：`context.run(coro.send, value)`。
- `throw(...)`：第一次 `send` 之前就被 `throw`（`task` 在第一次 `__step` 之前被取消），先 `F.cancel()`，这是原生任务已在 `F` 上等待时 `cancel()` 会做的事；然后 `context.run(coro.throw, ...)`。
- `close()`：`coro.close()`。
- `cr_frame`、`cr_running` 转发给 `coro`，所以 `task.get_stack()`、`task.print_stack()` 和任务的 `repr` 指向插件的协程。

loop 没有在运行时（peer 里不会出现），原生 eager start 退化成普通任务；`start` 也同样退化：直接 `asyncio.Task(rest, ...)`，不执行第一步，`_Rest` 的第一次 `send` 执行 `coro.send(None)`。

`_task` 改为：

```python
def _task(self, coroutine, call=None):
    context = contextvars.copy_context()
    if sys.version_info >= (3, 12):
        task = asyncio.Task(coroutine, loop=self.loop, context=context, eager_start=True)
    else:
        task = _eager.start(self.loop, coroutine, context)
    ...
```

### 3.3 contextvars

- 第一步和之后每一步都在同一个 `context`（`_task` 里 `copy_context()` 得到的那份）里执行，由 `_Rest` 用 `context.run` 保证。第一步里 `ContextVar.set` 的值，之后的步骤看得到；调用 `_task` 的一方看不到（它在自己的 context 里）。原型的 `s_suspend` 场景断言了这两点。
- 不把 `context` 交给 `asyncio.Task`：3.10 的 `Task` 没有 `context` 参数，构造时会复制当时的 context，复制发生在第一步之前，看不到第一步的修改；3.11 有这个参数，但交给 `Task` 后 `_Rest` 再 `context.run` 同一个 context 会报"已进入"。所以 `Task` 自己的 context 只用来运行它的 done 回调，插件代码一律在 `context` 里执行。3.10 和 3.11 走同一条代码。
- 调用链：`_execute` 在调用 `_task` 之前已经 `self._context.set(path)`，所以 `context` 里的 `rutis_path` 是这次调用的链。第一步期间 `self._sync_path` 也是这条链（`_execute` 设的），第一步里发出的同步调用带着它；之后的步骤在 loop 回调里执行，`_sync_path` 为 `None`，`_path()` 读 `context` 里的链。与原生相同。

### 3.4 第一步里的异常

| 第一步 | 原生 3.12 | 模拟 |
| --- | --- | --- |
| `return v` | 已完成的任务，结果 `v` | 已完成的 future，结果 `v` |
| 抛出普通异常 | 已完成的任务，带这个异常 | 已完成的 future，带这个异常 |
| 抛出 `CancelledError` | 已取消的任务 | 已取消的 future |
| 抛出 `KeyboardInterrupt` / `SystemExit` | 任务带上异常，异常继续向外抛出 `Task(...)` | future 不返回；`task` 带上异常，异常继续向外抛出 `start(...)` |

后一种情况下，异常从 `_task` 抛到 `_execute`，`_execute` 外层的 `except BaseException` 把它作为调用失败回复，两条路径相同。

### 3.5 取消

| 时机 | 发生什么（两条路径相同） |
| --- | --- |
| 方法开始之前 | 协议里不会发生：`cancel` 帧在 `invoke` 帧之后到达；如果 `invoke` 还在队列里（`call_soon` 的 `_run`），`_tasks` 里没有这次调用，`cancel` 只让 `Signal` 参数变为已取消，方法照常执行。现有行为，本文不改 |
| 第一步之中 | 只有第一步发出同步调用、在等待期间收到这次调用的 `cancel` 帧时才会发生。现在 `_tasks[call]` 在 `_task` 返回后才登记，这个 `cancel` 被忽略（代码阅读得出，原生路径同样如此）。见 3.5.1，本 PR 一起修 |
| 第一步之后、任务第一次 `__step` 之前 | 原生：任务已在 `F` 上等待，`cancel()` 取消 `F`，协程在 `await` 处收到 `CancelledError`。模拟：`task` 设 `_must_cancel`，第一次 `__step` 调 `_Rest.throw`，`_Rest` 先取消 `F` 再把 `CancelledError` 抛进协程。结果相同（原型 `s_cancel_now`：协程在 `await` 处收到取消，`F` 被取消） |
| 之后任意时刻 | 都是 `Task.cancel()` 的普通行为（原型 `s_cancel_later`） |
| 协程吞掉 `CancelledError` 并返回 | 任务正常完成（原型 `s_swallow_cancel`） |

#### 3.5.1 第一步之中的取消（本 PR 一起修）

做法：`_execute` 执行 `invoke` / `call` 期间把调用号放进 `self._starting`；`_receive` 处理 `cancel` 时，如果调用号在 `_starting` 里，记入 `self._cancel_early`；`_task` 得到未完成的任务后，如果调用号在 `_cancel_early` 里，立即 `task.cancel()`。协程在第一个挂起点收到 `CancelledError`。两条路径都适用。先写复现测试（6.2 第 5 条），再修。

### 3.6 返回的对象，调用方怎么等

- 第一步挂起：返回 `asyncio.Task`（与原生相同）。
- 第一步就结束：返回已完成的 `asyncio.Future`；原生返回已完成的 `asyncio.Task`。

peer 里对返回值只用 `asyncio.isfuture`、`done()`、`add_done_callback`、`cancel()`、`_outcome`（`_execute`、`_respond`、`_encode`、`_future_done`），`Future` 和 `Task` 都满足；回复都是 `future` 引用，Rust 侧 `await` 它。插件代码拿不到这个对象（它在 peer 内部）。

第一步就结束时，`start` 已经创建的 `task` 在下一轮 loop 以同样结果结束，然后被丢弃。它存在的原因是：第一步期间 `asyncio.current_task()` 必须是一个任务（3.8 节）。代价是每次调用多一个任务和一次调度（3.10 节）。

### 3.7 `loop.set_task_factory`

两条路径都直接构造 `asyncio.Task`，不使用工厂（原型 `s_factory`）。原因：原生 `asyncio.Task(..., eager_start=True)` 本来就不经过工厂；如果模拟经过工厂，工厂拿到的是 `_Rest` 而不是插件的协程，返回的任务可能不是 `asyncio.Task`，3.10 / 3.11 上的行为会和 3.12 不同。插件自己用 `create_task` 创建的任务照常使用工厂，不受影响。

`python.en.md` §3.3 第 2 条的建议保持：不全局安装 `eager_task_factory`，只对 peer 从返回的协程创建的任务使用 eager start。

### 3.8 `asyncio.current_task()`

第一步期间 `current_task()` 必须是将来驱动剩余部分的那个任务，否则第一步里的 `async with asyncio.timeout(...)`、`asyncio.TaskGroup()`（3.11 有）会因为"不在任务里"报错，或者取消错误的任务（第一步可能是在另一个任务的 `__step` 里、经同步调用的等待嵌套执行的，这时当前任务是外层任务）。

3.10、3.11 没有公开的切换当前任务的接口。`start` 直接改 `asyncio.tasks._current_tasks`（这两个版本的 C 实现和纯 Python 实现共用这个 dict，本机确认 `type(_current_tasks) is dict`），先取出外层任务，第一步后放回。不调用 `_enter_task` / `_leave_task`，因为外层任务存在时它们会报错。

这是私有接口，只在 3.10、3.11 上使用。这两个版本只接受安全修复（3.10 已停止维护），这部分代码不会再变。3.12 及以上走原生路径，不依赖它。原型在 3.12–3.14 上也能正确切换，测试可以在那里比较模拟与原生（6.1）。

### 3.9 与原生 3.12 的差异（写明并有界）

| 差异 | 影响 | 有界在哪 |
| --- | --- | --- |
| 第一步就结束时返回 `Future` 而不是 `Task` | peer 内部对象，只用 future 接口；插件看不到 | 测试断言 `isfuture` 和结果，不断言类型 |
| 第一步就结束时多一个在下一轮结束的任务；第一步里取得的 `current_task()` 是它 | 插件若在第一步里保存 `current_task()` 并在之后 `await` 它，得到同样的结果，只是晚一轮 | `asyncio.all_tasks()` 里短暂多一个任务 |
| 第一步挂起在未完成的 `F` 上时，唤醒在下一轮才登记到 `F` 上 | `F` 的回调里，任务唤醒的先后可能排在下一轮之前登记的其他回调之后；"什么时候继续"相同（`F` 完成后的下一轮） | 原型 `s_order_*` 三个场景的先后与原生一致；同一轮里回调的先后不作保证，文档不承诺 |
| `task.get_coro()` 返回 `_Rest` 而不是插件协程 | 只影响检查任务的工具 | `get_stack()` / `repr` 已转发到插件协程 |
| 每次调用多几微秒（3.10 节） | 同步往返本身约 25–60 µs | — |

### 3.10 开销（本机实测，Apple M 系列，每项 2 万次）

| 版本 | 模拟：第一步就结束 | 模拟：挂起一次 | 原生：第一步就结束 | 原生：挂起一次 |
| --- | --- | --- | --- | --- |
| 3.10.16 | 9.9 µs | 35.2 µs | — | — |
| 3.11.11 | 5.9 µs | 10.7 µs | — | — |
| 3.12.2 | 5.9 µs | 12.9 µs | 1.0 µs | 7.4 µs |
| 3.13.6 | 4.0 µs | 7.4 µs | 0.7 µs | 5.7 µs |

3.10 的"挂起一次"包含 3.10 自身较慢的 `sleep(0)` 和任务调度。和一次跨进程同步调用相比（`python.en.md` §3.2：Rust → Python 同步 invoke 49–59 µs）可以接受。

### 3.11 与策略 B（#228）的关系

策略 B 的"栈顺序"边：同一运行时里后开始执行的调用压在先开始的调用之上。按 eager start：

- `async def` 方法的第一步在 `_execute` 里、回复之前同步执行，和 `sync` 方法的函数体一样在栈上；第一步里发出的同步调用带着这次调用的链，Rust 看到的是"这次调用已经开始执行、还没返回"。
- 第一步之后回复 `future` 引用，对 Rust 来说这次 `invoke` 已经返回。剩余部分在 loop 回调里执行，不会嵌套在某个同步等待之上（同步等待期间 loop 只执行进来的帧，不执行任务的步骤）；但它发出的调用 `path` 里仍含这次调用的号。#228 实现时要处理"`path` 里含一个已返回的调用"：它不在栈上，不产生栈顺序边。这一点与 Python 版本无关，原生路径现在就是这样。
- 模拟在同一个位置（`_execute` → `_task` 之内）执行第一步，`_sync_path`、`_context`、回复时机都与原生相同，所以 3.10 / 3.11 上 Rust 看到的帧顺序与 3.12 相同，策略 B 不需要区分 Python 版本。"同步等待"字段只由 `_call_sync` 发送，与 eager start 无关。

结论：不需要改 #228 的设计；实现 #228 时注意"异步方法返回后，其后续调用的 `path` 仍含它"，本 PR 在 #228 里留一条评论。

## 四、调研表逐项

| 依赖 | 位置 | 3.10 上 | 处理 |
| --- | --- | --- | --- |
| `asyncio.Task(..., eager_start=True)` | `peer.py:915-916` | 不存在 | 3.12+ 保留；3.10 / 3.11 用 `_eager.start`（第三节） |
| `loop.create_task(coro, context=...)` | `peer.py:918` | 不存在（3.11 起） | 删除这条回退分支，由 `_eager.start` 取代；决定里的 `context.run(self.loop.create_task, coro)` 不再需要 |
| 单参数 `traceback.format_exception(exc)` | `peer.py:112` | 可用（3.10 起） | 不改 |
| `importlib.metadata.entry_points(group=, name=)` | `runner.py:379` | 可用（3.10 起） | 不改 |
| 运行时求值的 `dict \| None`、`list[str]` 注解 | `__main__.py:47, 72, 91, 118` | 可用（PEP 604 / 585 在 3.10 运行时可用） | 不改 |

另外在仓库里查了 3.11+ 才有的接口（`asyncio.timeout`、`TaskGroup`、`ExceptionGroup`、`except*`、`tomllib`、`typing.Self`、`StrEnum`、`asyncio.Runner`、`enterContext`、`add_note`），`python/`、模板和测试里都没有用到。模板 `crates/rutis-host/templates/python/` 的代码也只用 3.10 可用的写法。

`requires-python` 改为 `>=3.10`：`python/rutis/pyproject.toml:7`、`crates/rutis-host/pyproject.toml:13`、`crates/rutis-host/templates/python/pyproject.toml:6`。

低于 3.10 的解释器：pip / uv 按 `requires-python` 拒绝安装。不另加运行时的版本检查：绕过 `requires-python`（源码目录、`PYTHONPATH`）的用法很少见。

## 五、websocket：监听失败后 `accept()` 立即抛出（Q5.2.2）

### 5.1 现状

`python/rutis/rutis/websocket.py` `Listener`：

- `_serve` 线程运行 `serve_forever()`。websockets 15.0.1 的 `serve_forever()`（`websockets/sync/server.py` 第 274–281 行）在 `socket.accept()` 抛 `OSError`（如 `EMFILE`）时 `break`，**正常返回，不抛出**；只有选择器出错之类才抛出。
- 正常返回时 `_serve` 只设 `_stopped`，不记录失败；抛出时记入 `_failed` 并关闭监听套接字。
- `accept()` 是 `self._accepted.get()`，没有任何一种失败会唤醒它。

结果：`__main__.serve` 在 `asyncio.to_thread(listener.accept)` 上永远等，进程不结束也不报错；当前会话还能用，但控制方断线后无法重连。

另外 `__main__.main()` 是 `try: asyncio.run(...) finally: ... os._exit(0)`：`run` 抛出的异常在打印之前进程就以 0 退出，所以即使 `accept()` 抛出，失败也不可见。

### 5.2 修改

```python
_STOPPED = object()

def _serve(self):
    try:
        self._server.serve_forever()
        with self._lock:
            closing = self._closing
        if not closing:
            # serve_forever() returns when accept() fails (EMFILE, ENFILE,
            # ENOBUFS...); websockets swallows the error itself.
            self._failed.append(OSError("the WebSocket listener stopped: accepting a connection failed "
                                        "(for example, out of file descriptors)"))
            self._server.socket.close()
    except BaseException as error:
        ...                      # 现有处理不变
    finally:
        self._stopped.set()
        self._accepted.put(_STOPPED)

def accept(self):
    item = self._accepted.get()
    if item is _STOPPED:
        self._accepted.put(_STOPPED)          # 之后的 accept() 也立即返回
        if self._failed:
            raise self._failed[0]
        raise ConnectionError("the listener is closed")
    return item
```

- 原始 errno 被 websockets 吞掉，拿不到；错误消息写明可能的原因。
- `close()` 之后的 `accept()` 也立即抛出（现在同样永远阻塞）。`listen_once` 先 `accept()` 再 `close()`，不受影响。
- `_STOPPED` 之后才进队列的连接（处理线程在停止前已开始）：`close()` 和失败路径都不再有人接受它们，`accept()` 遇到 `_STOPPED` 时把队列里剩下的通道用 `GOING_AWAY` 关掉，不留挂着的连接（Q2.4）。

`__main__`：

- `serve()` 里 `accepting.result()` 抛出时：结束当前会话（`await current.end()`，清理租约），然后把异常抛出 `serve()`。
- `main()`：`run` 抛出时把 traceback 打到 stderr，以退出码 1 结束；正常结束仍是 0。`os._exit` 保留（插件线程不能拖住进程）。

当前会话是否等它自然结束再退出，已决定立即结束（第十三节）：一个不能再接受重连的运行时应当让外部的进程管理（systemd、容器）知道并重启它。

## 六、测试

所有测试遵守 PR 的工作约定：不用 sleep 做同步，超时只防挂死（≥ 10 s），后台线程和任务的异常让测试失败（`tests/background.py`）。

### 6.1 eager start 等价性：`python/rutis/tests/test_eager.py`（新）

一组场景，每个场景有**写死的预期记录**（不是"两种实现结果相同"，而是"每种实现都等于预期"）：

| 场景 | 断言 |
| --- | --- |
| 从不挂起，返回值 | 创建后立即完成，结果正确；前半段已执行 |
| 第一步抛异常 / 抛 `CancelledError` | 立即完成，带异常 / 已取消 |
| 第一步挂起在 future 上 | 创建后未完成；第一步设的 contextvar 在后续可见、调用方不可见；结果正确 |
| `current_task()` | 第一步里是返回的任务；之后恢复外层任务；在另一个任务的步骤里创建时同样成立 |
| `asyncio.timeout` 在第一步里（3.11+） | 正常超时，不报"不在任务里" |
| 创建后立即取消 / 几轮后取消 / 吞掉取消 | 协程在 `await` 处收到 `CancelledError`，被等的 future 被取消；吞掉时任务正常完成 |
| `sleep(0)` 与其他回调的先后、`F` 已完成 / 稍后完成时继续的先后 | 记录顺序等于预期 |
| 第一步之后抛异常 | 任务带异常 |
| 设置了任务工厂 | 工厂没有被调用 |
| `get_stack()` | 顶层帧是插件协程 |
| 没有被取走的异常 | 不产生 asyncio 日志（捕获 `asyncio` logger） |

运行方式：

- 所有版本：`_eager.start` 跑全部场景，等于预期；
- 3.12+：原生 `asyncio.Task(..., eager_start=True)` 跑全部场景，等于同一份预期；
- 3.12+ 上 `_eager.start` 也跑（防止预期写错）。如果将来某个版本改变了 `_current_tasks`，只在该版本上跳过模拟，跳过原因写明（Q7.8）；产品代码在 3.12+ 不用模拟。

3.10 没有 `asyncio.timeout`，该场景在 3.10 上跳过并写明。

原型（第十二节）的 17 个场景：3.10.16、3.11.11 上模拟的记录与 3.12+ 上原生的记录相同；3.12.2、3.13.6、3.14.0a5 上模拟与原生逐项相同。

### 6.2 async 方法经过 peer：`python/rutis/tests/test_peer.py`（补）

用现有的 socketpair 脚本对端：

1. **同步链上 await 一个从不挂起的 async 方法**（风险 P10 / 本 issue 的回归）：`_waiting=["node:1"]` 时收到 `invoke rust:1`（`path` `["node:1"]`），方法是不挂起的 `async def`；再收到 `await` 它返回的引用（同一链）。预期 `return`，不是 `SyncWaitCycle`。3.10 / 3.11 上如果退回懒启动，这个测试失败。
2. **前半段在回复之前执行**：方法第一步记录一个标记；读到 `return` 帧时标记已存在。
3. **第一步里同步回调**：方法第一步 `peer.call(...)` 同步调用对端，对端回复后方法继续；回复帧的顺序是"回调请求 → 回调结果 → 方法的 `return`"。
4. **取消**：方法挂起在门控 future 上；对端发 `cancel`；协程收到 `CancelledError`，`await` 得到 `throw` `CancelledError`。另一例：`invoke` 和 `cancel` 在同一批帧里送达（任务第一次 `__step` 之前）。
5. **第一步之中的取消**（3.5.1）：方法第一步同步调用对端，对端在回复之前发这次调用的 `cancel`；协程在第一个挂起点收到 `CancelledError`。
6. **异常**：第一步抛出 → `invoke` 的回复是 `return` 一个已完成的 future 引用，`await` 它得到 `throw`，`name` 是异常类名；挂起后抛出 → 同样。
7. **调用链**：方法 `await` 之后发出的调用，`path` 含这次调用的号。

### 6.3 websocket：`python/rutis/tests/test_websocket.py`（补）

1. `serve_forever` 被替换为立即返回（模拟 `accept()` 失败）：另一线程里的 `accept()` 在 10 s 内抛 `OSError`，消息含"stopped"；`close()` 抛同一个错误；之后的 `dial` 被拒绝。
2. `close()` 之后 `accept()` 立即抛 `ConnectionError`。
3. 真实的 `EMFILE`（Linux、macOS；Windows 跳过并写明）：子进程里降低 `RLIMIT_NOFILE`、占满描述符后监听，父进程连接，子进程的 `accept()` 抛出、进程以非 0 退出、stderr 含原因。
4. `__main__.serve`：用替换过的 `websocket.listen` 让 `accept()` 失败，`serve()` 抛出，当前会话的 `end()` 已执行。

### 6.4 Rust 侧

不新增 Rust 测试的必要条件：现有的 loader / bridge 测试在 3.10 上跑（CI 的 `rust` 任务，第七节）。本机 3.10 上 `multilang`、`instance_runtimes`、`python_rows` 在改前全部失败、改后全部通过，说明它们已经覆盖 async 方法。

建议加一个 bridge 测试，固定 2.2 的语义，与 Python 版本无关：`rutis-bridge/tests/python_runtime.rs` `an_async_python_method_that_never_suspends_is_ready_inside_a_sync_chain`：Rust 在一次同步调用的链上调用 Python 的不挂起 `async def` 方法并 `await` 结果，得到结果而不是 `SyncWaitCycle`。

### 6.5 安装冒烟

#193（E2E S9）的 PyPI 安装冒烟用 3.10：`uv venv -p 3.10`，安装 `rutis-host`，`uvx rutis-host new` 生成的 Python 插件能跑通。这是 #196 的验收之一，由 #193 的任务执行。

## 七、CI（Linux 的 Python 版本由本 PR 改，其余由 #256 改）

按 #204 的范围调整：PR 上不新增任务，替换版本。

| 任务 | 机器 | 现在 | 改为 |
| --- | --- | --- | --- |
| `rust` | Linux | 3.12 | **3.10**（最低版本；loader / bridge / host 全部 Rust 测试经过模拟路径） |
| `js-py` | Linux | 3.12 | **3.10**（Python 单元测试） |
| `network-macos` | macOS | 3.12 | **最新版**（原生路径；Python 单元测试与平台相关的 Rust 测试） |
| `runtimes-windows` | Windows | 3.12 | 最新版 |
| `runtimes-go`、`runtimes-bun` | Linux / macOS | 3.12 | Linux 3.10，macOS 最新版（Python 只是依赖，跟随所在平台） |

- websockets：Linux 装 `websockets==15.*`，macOS / Windows 装最新版（#204 的表）。
- "最新版"怎么写：写作 `'3.x'`，由 #256 改。
- 不额外跑 3.11、3.12：Q10.2 只要求最低和最新，与 #204 的规则一致。3.11 与 3.10 走同一条模拟路径；6.1 的测试在开发机上用 3.11、3.12 跑过（第十二节）。
- `docs/ci.md` / `ci.en.md` 的版本说明随 `ci.yml` 一起由 CI 那边的 PR 更新。

## 八、文档

中英文同步：

| 文件 | 改动 |
| --- | --- |
| `docs/guide/python-plugin.md` / `.en.md` 第 3 行 | "需要 Python 3.10 或更高" |
| `docs/guide/README.md` / `.en.md` 第 33 行 | 同上 |
| `docs/design-philosophy.md` / `.en.md` §六 | "Python 3.10+" |
| `docs/design-multilang-runtimes-2026-10-03.md` / `.en.md` 第 148 行 | "需要 Python 3.10 或更高（3.10 / 3.11 上 eager start 由 rutis 实现，见本文）" |
| `docs/design-developer-packages-2026-10-06.md` / `.en.md` 第 278 行 | 3.10 及以上 |
| `docs/guide/plugin-api.md` / `.en.md` "同步调用与可重入" | 加一句：`async` 方法在回复之前执行到第一个 `await`；从不挂起的 `async` 方法在同步链上也能被等待；各 Python 版本相同 |
| `python/rutis/README.md` | "Python 3.10 or later" |
| `crates/rutis-bridge/src/runtime/local.rs:62` 文档注释 | 3.10 |
| 下一版发布说明（0.9） | Python 最低版本降到 3.10 |

不改：历史发布说明（`docs/releases/0.7.0*`、`0.8.0*`）、迁移文档、`docs/plan/analysis/multilang/python*.md`（调研记录，保留当时的结论）、`docs/quality-status.md`（每一步结束时统一更新，届时 Q10.2 / 风险 MX 写入"Python 3.10 与最新版已验证"）。

## 九、风险

| 风险 | 后果 | 处理 |
| --- | --- | --- |
| 模拟与原生有未发现的差异 | 3.10 / 3.11 上个别插件的时序与 3.12 不同 | 6.1 的固定预期在三个版本段上都断言；Rust 侧全部测试在 3.10 上跑 |
| 使用私有的 `asyncio.tasks._current_tasks` | 只在 3.10 / 3.11 上用，这两个版本不再有功能变更 | 3.12+ 不用；测试在 3.12+ 上若发现它失效只跳过模拟测试 |
| 第一步就结束时每次调用多一个任务 | 每次约 4–10 µs | 3.10 节实测；调用频繁的场景本来就建议放到同一运行时或 Rust |
| 3.10 已停止维护 | 安全修复只来自发行版（Ubuntu 22.04 等） | 目标用户的环境决定；3.10 在 CI 里一直验证，停止验证时按 Q10.3 从声明中删除 |
| websockets 吞掉 `accept()` 的 errno | 错误消息只能写可能原因 | 消息写明；测试 6.3 第 3 条确认真实 `EMFILE` 走到这条路径 |
| 监听失败时直接结束进程 | 当前会话随之断开 | 第十三节第 1 条；结束前清理租约，退出码非 0 |

## 十、分阶段（都在本 PR 内）

1. **eager start**：`_eager.py`、`_task` 改动、`test_eager.py`、`test_peer.py` 的 6.2 测试；3.10 / 3.11 / 3.12+ 本地全部通过。
2. **版本声明**：三处 `requires-python`；第八节文档。
3. **websocket 与 `__main__`**：第五节修改和 6.3 测试。
4. **CI**：本 PR 把 Linux 任务的 Python 换成 3.10，和 `requires-python` 的修改在同一个 PR 里，保证 Q10.4（先验证再声明）；macOS、Windows 的版本由 #256 改。

## 十一、验收（都可自动检查）

1. `python3.10 -m unittest discover -s tests`、`python3.11 …`、`python3.12+ …`（`python/rutis`）全部通过，`test_eager.py` 的每个场景在每个版本上都与预期一致（3.10 的 `asyncio.timeout` 场景按写明的原因跳过）。
2. CI `rust` 任务在 Python 3.10、websockets 15 上通过，包括 `rutis-loader --features node,python,go,peer` 与 `rutis-bridge --all-features` 的全部测试和 loopback 重跑。
3. CI `network-macos` 在最新 Python 上通过。
4. `grep -rn "3\.12" python crates/rutis-host/pyproject.toml crates/rutis-host/templates docs/guide` 只剩说明原生 eager start 的地方。
5. 6.3 的 websocket 测试：监听失败后 `accept()` 在 10 s 内抛出；`python -m rutis listen:…` 以非 0 退出。
6. #193 的 S9 PyPI 安装冒烟在 3.10 上通过。

## 十二、本地验证记录（2026-10-11，macOS arm64）

验证过的：

- 原型 `_eager.py` 与 17 个场景（6.1 的表）：3.10.16、3.11.11 上模拟结果符合预期；3.12.2、3.13.6、3.14.0a5 上模拟与原生逐项相同。
- `type(asyncio.tasks._current_tasks) is dict`，且 `asyncio.Task is _asyncio.Task`（C 实现）：3.10–3.14 均成立。
- 3.10.16 + websockets 15.0.1：`python/rutis` 41 个单元测试通过（未改代码）。
- 3.10.16：未改代码时 `multilang`（4）、`instance_runtimes`（2，另 1 个与 Python 无关的通过）、`python_rows`（1）失败；把原型接入 `_task` 后全部通过。
- 3.10.16 + websockets 15.0.1，原型接入 `_task`（临时改动，未提交）：`cargo test -p rutis-loader --features node,python,peer` 与 `cargo test -p rutis-bridge --all-features` 全部通过（含 `leases`、`python_runtime`、`runtime_conformance`、`session_matrix`、`rpc_callbacks`、`cancellation`、WebSocket 各项），没有失败。Go 行的 loader 测试未跑（没开 `go` feature）。
- websockets 15.0.1 源码：`serve_forever()` 在 `accept()` 抛 `OSError` 时返回。
- 开销：3.10 节。

没有验证的：

- Linux 上的上述结果（CI 会做）；
- Windows；
- 3.15（本机没有）；
- websockets 16 / 17 的 `serve_forever()` 在 `accept()` 失败时的行为（实现时读源码并加进 6.3 的测试矩阵）；
- 3.5.1 的"第一步之中的取消被忽略"只来自代码阅读，实现时先写测试复现。

## 十三、维护者的决定（2026-10-11）

- 监听失败后立即结束当前会话，清理租约后以非 0 退出，交给外部进程管理重启（第五节）。
- 第一步之中到达的 `cancel` 被忽略的问题在本 PR 一起修（3.5.1）。
- 3.12+ 继续用原生 eager start。
- `ci.yml` 里 Linux 的 Python 换成 3.10 由本 PR 自己改（"`ci.yml` 只由 #203 / #204 修改"的例外）；macOS、Windows 用最新版，写作 `3.x`，由 #256 改。
- 不加任务名，不加运行时版本检查，CI 不额外跑 3.11、3.12。
