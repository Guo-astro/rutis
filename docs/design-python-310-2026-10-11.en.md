# Python 3.10 support with our own eager start (design draft)

[中文](design-python-310-2026-10-11.md)

Status: design, under review. Date: 2026-10-11. Base: `main` `0941ef4`.
Sources: #196 (including the 2026-10-09 decision and the `accept()` leftover from #218), #204 (CI scope note), #197 / #218 (websockets lower bound 15), #193 (PyPI install smoke), #228 (strategy B). Part of step 1 of #183.
Standard: [quality standard](quality-standard.en.md) Q10.2, Q10.4, Q6.7.1, Q5.2.2; [status](quality-status.en.md) risk MX.

Out of scope: Python 3.9 and older; the Node minimum version (#195); the websockets lower bound itself (#197 is merged); the other CI version changes (made by #204, #256).

## 1. Decisions

| # | Decision |
| --- | --- |
| D1 | Minimum Python 3.10; `requires-python = ">=3.10"` (three places). 3.9 is not supported |
| D2 | 3.12 and later keep the native `asyncio.Task(..., eager_start=True)` |
| D3 | 3.10 and 3.11 emulate eager start in a new module `rutis/_eager.py`, behaving like native (section 3); differences are listed and bounded (3.9) |
| D4 | Both paths construct `asyncio.Task` directly, not through `loop.create_task`, so neither uses a factory set with `loop.set_task_factory` (as the 3.12 path does today) |
| D5 | Both paths share one set of tests with fixed expectations, asserting the same results on 3.10, 3.11 and 3.12+ |
| D6 | Once a `websocket.Listener` fails, `accept()` raises at once; `python -m rutis` then exits with a non-zero code and prints the reason |
| D7 | CI: jobs on Linux use 3.10, jobs on macOS the latest release; no new PR jobs (consistent with #204) |

## 2. Current state

### 2.1 What depends on 3.12

`python/rutis/rutis/peer.py` `Peer._task` (lines 911–922):

```python
context = contextvars.copy_context()
if sys.version_info >= (3, 12):
    task = asyncio.Task(coroutine, loop=self.loop, context=context, eager_start=True)
else:
    task = self.loop.create_task(coroutine, context=context)
```

`_task` has two callers:

- `_execute` (line 890): an incoming `invoke` / `call` returned a coroutine (an `async def` method); before `_execute` replies, the coroutine becomes a task, and `_respond` replies with the task as a `future` reference;
- `_encode` (line 498): a coroutine is encoded as a value (for example inside a return value).

Both run on the loop thread with the loop running. When `_execute` calls `_task`, `self._context` (the call chain `path`) is already set to this call's chain, and so is `self._sync_path`; `copy_context()` carries the chain into the task, so calls made by later steps carry this call's id in their `path`.

### 2.2 Why eager start is needed

`docs/plan/analysis/multilang/python.en.md` §3.3 item 2: a Python coroutine does not run until it is scheduled; a JS async function runs its synchronous prefix immediately. Eager start makes `async def` methods behave like JS:

1. the prefix runs before the reply;
2. a coroutine that never suspends is already done when its task is created.

Point 2 decides whether `SyncWaitCycle` is reported falsely. When `_execute` handles an `await` frame (lines 864–876): if the awaited future is done, it replies with the result; if not, and the `await` belongs to a chain Python is synchronously waiting on, it replies `SyncWaitCycle`, because the loop is held by the synchronous call and the future cannot finish during the wait. Without eager start, the task returned by an `async def` method that never suspends only runs on the next loop iteration, so awaiting it on a synchronous chain finds it "not done" and gets `SyncWaitCycle`; with eager start it is already done and returns normally. The unit tests `test_a_finished_future_is_ready_during_a_synchronous_call` and `test_a_pending_future_on_the_chain_is_a_cycle` cover the two branches, but with hand-made futures that never go through `_task`.

### 2.3 Measured on 3.10 (locally, repository unchanged)

- `python/rutis` unit tests: on 3.10.16 (with websockets 15.0.1) all 41 pass; on 3.11.11 (without websockets) they pass with 9 skipped.
- Rust side: `cargo test -p rutis-loader --features node,python --test python_rows --test multilang --test instance_runtimes` with `python3` pointing at 3.10.16: 7 of 8 tests fail (the one that passes does not start Python), with `BaseEventLoop.create_task() got an unexpected keyword argument 'context'` and `coroutine 'Runtime.load' was never awaited`. This matches the survey in #196: the Python unit tests never go through `_task`.
- With the prototype from section 3 (`_eager.py`) replacing the pre-3.12 branch, the same tests pass on 3.10.16, and so do all loader (`node,python,peer`) and bridge (all features) tests; see section 12.

## 3. Emulating eager start

### 3.1 Native 3.12 behaviour (the parts we rely on)

`asyncio.Task(coro, loop=loop, context=ctx, eager_start=True)` with the loop running:

1. makes the new task the current task, runs the coroutine's first step synchronously in `ctx`, then restores the previous current task;
2. if the first step finishes (returns or raises), the task is done and never scheduled;
3. if the first step suspends on a future `F`, it registers its wakeup on `F` immediately, and from then on is an ordinary task; if it suspends on `None` (`await asyncio.sleep(0)`), it continues on the next loop iteration;
4. every later step runs in `ctx`;
5. it does not go through the task factory.

### 3.2 Algorithm

New module `python/rutis/rutis/_eager.py`, used by `_task` only on 3.10 and 3.11 (tests use it on every version, see 6.1):

```python
def start(loop, coro, context):
    """asyncio.Task(coro, loop=loop, context=context, eager_start=True)
    for Python 3.10 and 3.11."""
    rest = _Rest(coro, context)
    task = asyncio.Task(rest, loop=loop)   # schedules the first __step; no factory
    previous = _tasks._current_tasks.get(loop)
    _tasks._current_tasks[loop] = task                # current_task() is task during the first step
    try:
        try:
            rest.waited = context.run(coro.send, None) # the first step
            return task                                # suspended: task drives the rest
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
    # Done in the first step: return a completed future. task finishes with
    # the same outcome on its first __step; a done callback retrieves its
    # exception so it is not logged twice.
    task.add_done_callback(_retrieve)
    return _completed(loop, rest.outcome)
```

`_Rest` implements the coroutine protocol (`send`, `throw`, `close`, registered as `collections.abc.Coroutine`) and is driven by `task`. It stands for "the rest after the first step":

- first `send`:
  - the first step finished: raise `StopIteration(value)` or the exception from `outcome`, so `task` ends with the same result;
  - the first step suspended on `None`: one loop iteration has passed, so continue directly with `context.run(coro.send, None)`;
  - the first step suspended on a future `F` that is already done: set `F._asyncio_future_blocking` back to `False` and continue directly with `context.run(coro.send, None)` (`Future.__await__` returns `F.result()`, which raises `CancelledError` at the `await` if `F` was cancelled);
  - otherwise: hand `F` to `task` (as the return value of `send`); `task` registers its wakeup and runs its usual checks (same loop, a legal yield).
- every later `send(value)`: `context.run(coro.send, value)`.
- `throw(...)`: if it comes before the first `send` (`task` was cancelled before its first `__step`), first `F.cancel()` — what `cancel()` does to a native task already waiting on `F` — then `context.run(coro.throw, ...)`.
- `close()`: `coro.close()`.
- `cr_frame` and `cr_running` delegate to `coro`, so `task.get_stack()`, `task.print_stack()` and the task's `repr` point at the plugin's coroutine.

If the loop is not running (never the case in the peer), native eager start falls back to an ordinary task; `start` does the same: it only constructs `asyncio.Task(rest, ...)` without running the first step, and `_Rest`'s first `send` calls `coro.send(None)`.

`_task` becomes:

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

- The first step and every later step run in the same `context` (the one `_task` gets from `copy_context()`); `_Rest` ensures it with `context.run`. A value the first step sets with `ContextVar.set` is visible to later steps and not to the caller of `_task` (which runs in its own context). The prototype's `s_suspend` scenario asserts both.
- `context` is not passed to `asyncio.Task`: on 3.10 `Task` has no `context` parameter and copies the current context at construction, which happens before the first step, so it would miss the first step's changes; 3.11 has the parameter, but if `Task` enters `context`, `_Rest` entering the same context again fails with "already entered". So the `Task`'s own context only runs its done callbacks; plugin code always runs in `context`. 3.10 and 3.11 take the same code path.
- Call chain: `_execute` calls `self._context.set(path)` before `_task`, so `rutis_path` in `context` is this call's chain. During the first step `self._sync_path` is also that chain (set by `_execute`), so synchronous calls made in the first step carry it; later steps run from loop callbacks with `_sync_path` `None`, and `_path()` reads the chain from `context`. Same as native.

### 3.4 Exceptions in the first step

| First step | Native 3.12 | Emulation |
| --- | --- | --- |
| `return v` | a done task with result `v` | a done future with result `v` |
| raises an ordinary exception | a done task holding the exception | a done future holding the exception |
| raises `CancelledError` | a cancelled task | a cancelled future |
| raises `KeyboardInterrupt` / `SystemExit` | the task holds it and it propagates out of `Task(...)` | no future is returned; `task` holds it and it propagates out of `start(...)` |

In the last case the exception propagates from `_task` into `_execute`, whose outer `except BaseException` replies with it as the call's failure; both paths are the same.

### 3.5 Cancellation

| When | What happens (same on both paths) |
| --- | --- |
| Before the method starts | Does not happen in the protocol: a `cancel` frame arrives after its `invoke` frame; if the `invoke` is still queued (`_run` via `call_soon`), `_tasks` has no entry for the call, so the `cancel` only marks a `Signal` argument cancelled and the method runs. Existing behaviour, unchanged here |
| During the first step | Only if the first step makes a synchronous call and, while waiting, receives a `cancel` frame for this call. `_tasks[call]` is only recorded after `_task` returns, so that `cancel` is ignored today (found by reading the code; the native path is the same). See 3.5.1; fixed in this PR |
| After the first step, before the task's first `__step` | Native: the task already waits on `F`; `cancel()` cancels `F`, and the coroutine gets `CancelledError` at its `await`. Emulation: `task` sets `_must_cancel`; its first `__step` calls `_Rest.throw`, which cancels `F` and throws `CancelledError` into the coroutine. Same result (prototype `s_cancel_now`: the coroutine is cancelled at its `await`, `F` is cancelled) |
| Any later time | Ordinary `Task.cancel()` behaviour (prototype `s_cancel_later`) |
| The coroutine swallows `CancelledError` and returns | The task completes normally (prototype `s_swallow_cancel`) |

#### 3.5.1 Cancellation during the first step (fixed in this PR)

While `_execute` runs an `invoke` / `call`, put the call id in `self._starting`; when `_receive` handles `cancel` for an id in `_starting`, add it to `self._cancel_early`; when `_task` gets a task that is not done and its call id is in `_cancel_early`, call `task.cancel()` at once. The coroutine gets `CancelledError` at its first suspension point. Applies to both paths. A test reproduces it first (6.2 item 5), then the fix.

### 3.6 The returned object, and how callers await it

- The first step suspended: an `asyncio.Task` (as native).
- The first step finished: a completed `asyncio.Future`; native returns a completed `asyncio.Task`.

The peer only uses `asyncio.isfuture`, `done()`, `add_done_callback`, `cancel()` and `_outcome` on it (`_execute`, `_respond`, `_encode`, `_future_done`); `Future` and `Task` both satisfy that, and the reply is a `future` reference either way, which the Rust side awaits. Plugin code never sees this object (it stays inside the peer).

When the first step finished, the `task` that `start` already created finishes with the same result on the next loop iteration and is then dropped. It exists because `asyncio.current_task()` must be a task during the first step (3.8). The cost is one extra task and one scheduling per call (3.10).

### 3.7 `loop.set_task_factory`

Neither path uses the factory (prototype `s_factory`). Reason: native `asyncio.Task(..., eager_start=True)` never goes through the factory; if the emulation did, the factory would receive `_Rest` instead of the plugin's coroutine and could return something other than an `asyncio.Task`, and 3.10 / 3.11 would behave differently from 3.12. Tasks plugins create themselves with `create_task` still use the factory.

The advice in `python.en.md` §3.3 item 2 stands: do not install `eager_task_factory` globally; use eager start only for tasks the peer creates from returned coroutines.

### 3.8 `asyncio.current_task()`

During the first step `current_task()` must be the task that will drive the rest. Otherwise `async with asyncio.timeout(...)` or `asyncio.TaskGroup()` (3.11) in the first step fails with "not inside a task", or cancels the wrong task (the first step can run nested inside another task's `__step` through a synchronous call's wait, where the current task is that outer task).

3.10 and 3.11 have no public way to switch the current task. `start` writes `asyncio.tasks._current_tasks` directly (on these versions the C and pure-Python implementations share this dict; verified locally that `type(_current_tasks) is dict`): it takes out the outer task and puts it back after the first step. It does not call `_enter_task` / `_leave_task`, which raise when an outer task is current.

This is a private interface, used only on 3.10 and 3.11. Those versions only receive security fixes (3.10 has reached end of life), so this code will not change under us. 3.12 and later take the native path and do not depend on it. The prototype also switches correctly on 3.12–3.14, so tests can compare emulation and native there (6.1).

### 3.9 Differences from native 3.12 (listed and bounded)

| Difference | Effect | Bound |
| --- | --- | --- |
| A `Future` rather than a `Task` when the first step finishes | An object internal to the peer, used only through the future interface; plugins never see it | Tests assert `isfuture` and the result, not the type |
| When the first step finishes, an extra task that finishes one iteration later; `current_task()` taken in the first step is that task | A plugin that keeps `current_task()` from the first step and later awaits it gets the same result, one iteration later | `asyncio.all_tasks()` briefly shows one more task |
| When the first step suspends on a pending `F`, the wakeup is registered on `F` one iteration later | Among `F`'s callbacks, the task's wakeup may come after callbacks others registered before that iteration; *when* it continues is the same (the iteration after `F` completes) | The prototype's three `s_order_*` scenarios order exactly as native; ordering among callbacks of the same iteration is not guaranteed, and the docs do not promise it |
| `task.get_coro()` returns `_Rest` rather than the plugin coroutine | Only affects tools that inspect tasks | `get_stack()` / `repr` delegate to the plugin coroutine |
| A few microseconds more per call (3.10) | A synchronous round trip itself is about 25–60 µs | — |

### 3.10 Cost (measured locally, Apple M series, 20,000 runs each)

| Version | Emulated: done in first step | Emulated: suspends once | Native: done in first step | Native: suspends once |
| --- | --- | --- | --- | --- |
| 3.10.16 | 9.9 µs | 35.2 µs | — | — |
| 3.11.11 | 5.9 µs | 10.7 µs | — | — |
| 3.12.2 | 5.9 µs | 12.9 µs | 1.0 µs | 7.4 µs |
| 3.13.6 | 4.0 µs | 7.4 µs | 0.7 µs | 5.7 µs |

On 3.10, "suspends once" includes 3.10's own slower `sleep(0)` and task scheduling. Compared with one cross-process synchronous call (`python.en.md` §3.2: Rust → Python sync invoke 49–59 µs), this is acceptable.

### 3.11 Interaction with strategy B (#228)

Strategy B's "stack order" edge: in one runtime, a call that starts later sits above one that started earlier. With eager start:

- The first step of an `async def` method runs synchronously inside `_execute`, before the reply, on the stack just like the body of a `sync` method; synchronous calls it makes carry this call's chain, and Rust sees "this call has started and has not returned".
- After the first step, the reply is a `future` reference; for Rust, this `invoke` has returned. The rest runs from loop callbacks and never nests above a synchronous wait (during a synchronous wait the loop only runs incoming frames, not task steps), but the calls it makes still carry this call's id in their `path`. #228 must handle "a `path` that names a call which has already returned": it is not on the stack and yields no stack-order edge. This does not depend on the Python version; the native path already behaves so.
- The emulation runs the first step at the same place (inside `_execute` → `_task`), with the same `_sync_path`, `_context` and reply timing as native, so on 3.10 / 3.11 Rust sees the same frame order as on 3.12, and strategy B need not distinguish Python versions. The "waiting synchronously" field is sent only by `_call_sync` and is unrelated to eager start.

Conclusion: #228's design needs no change; when implementing #228, note that "after an async method has returned, the `path` of its later calls still names it". This PR leaves a comment on #228.

## 4. The survey table, item by item

| Dependency | Where | On 3.10 | Handling |
| --- | --- | --- | --- |
| `asyncio.Task(..., eager_start=True)` | `peer.py:915-916` | missing | kept on 3.12+; `_eager.start` on 3.10 / 3.11 (section 3) |
| `loop.create_task(coro, context=...)` | `peer.py:918` | missing (3.11+) | the fallback branch is removed and replaced by `_eager.start`; the decision's `context.run(self.loop.create_task, coro)` is no longer needed |
| single-argument `traceback.format_exception(exc)` | `peer.py:112` | available (3.10+) | unchanged |
| `importlib.metadata.entry_points(group=, name=)` | `runner.py:379` | available (3.10+) | unchanged |
| runtime-evaluated `dict \| None`, `list[str]` annotations | `__main__.py:47, 72, 91, 118` | available (PEP 604 / 585 work at runtime on 3.10) | unchanged |

We also searched the repository for 3.11+ interfaces (`asyncio.timeout`, `TaskGroup`, `ExceptionGroup`, `except*`, `tomllib`, `typing.Self`, `StrEnum`, `asyncio.Runner`, `enterContext`, `add_note`): none is used under `python/`, in the template or in the tests. The template under `crates/rutis-host/templates/python/` also only uses what 3.10 has.

`requires-python` becomes `>=3.10` in `python/rutis/pyproject.toml:7`, `crates/rutis-host/pyproject.toml:13` and `crates/rutis-host/templates/python/pyproject.toml:6`.

Interpreters older than 3.10: pip / uv refuse to install because of `requires-python`. No extra runtime version check: uses that bypass `requires-python` (source checkouts, `PYTHONPATH`) are rare.

## 5. websocket: `accept()` raises once listening has failed (Q5.2.2)

### 5.1 Current state

`python/rutis/rutis/websocket.py` `Listener`:

- The `_serve` thread runs `serve_forever()`. In websockets 15.0.1, `serve_forever()` (`websockets/sync/server.py` lines 274–281) `break`s when `socket.accept()` raises `OSError` (such as `EMFILE`) and **returns normally without raising**; it only raises for failures such as the selector's.
- On a normal return, `_serve` only sets `_stopped` and records no failure; on an exception it records it in `_failed` and closes the listening socket.
- `accept()` is `self._accepted.get()`; no failure wakes it.

Result: `__main__.serve` waits forever on `asyncio.to_thread(listener.accept)`; the process neither ends nor reports an error. The current session still works, but once the controller disconnects it cannot reconnect.

Also, `__main__.main()` is `try: asyncio.run(...) finally: ... os._exit(0)`: an exception from `run` exits the process with 0 before it is printed, so even if `accept()` raised, the failure would be invisible.

### 5.2 Changes

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
        ...                      # existing handling unchanged
    finally:
        self._stopped.set()
        self._accepted.put(_STOPPED)

def accept(self):
    item = self._accepted.get()
    if item is _STOPPED:
        self._accepted.put(_STOPPED)          # later accept() calls return at once too
        if self._failed:
            raise self._failed[0]
        raise ConnectionError("the listener is closed")
    return item
```

- websockets swallows the original errno; it cannot be recovered, so the message names the likely cause.
- `accept()` after `close()` also raises at once (today it blocks forever too). `listen_once` accepts before it closes and is unaffected.
- Connections queued behind `_STOPPED` (their handler threads started before the stop): nobody will accept them after `close()` or a failure; when `accept()` meets `_STOPPED` it closes the remaining channels in the queue with `GOING_AWAY`, so no connection is left hanging (Q2.4).

`__main__`:

- In `serve()`, when `accepting.result()` raises: end the current session (`await current.end()`, cleaning up its lease), then let the exception propagate out of `serve()`.
- `main()`: when `run` raises, print the traceback to stderr and exit with code 1; a normal end is still 0. `os._exit` stays (plugin threads must not keep the process alive).

The current session does not wait to end on its own; it ends at once (section 13). A runtime that can no longer accept a reconnection should let its process manager (systemd, a container runtime) see that and restart it.

## 6. Tests

All tests follow the PR's working rules: no sleeps for synchronisation, timeouts only against hangs (≥ 10 s), exceptions in background threads and tasks fail the test (`tests/background.py`).

### 6.1 Eager-start equivalence: `python/rutis/tests/test_eager.py` (new)

A set of scenarios, each with a **fixed expected record** (not "both implementations agree" but "each implementation equals the expectation"):

| Scenario | Assertion |
| --- | --- |
| Never suspends, returns a value | done right after creation, correct result; the prefix ran |
| First step raises / raises `CancelledError` | done at once, with the exception / cancelled |
| First step suspends on a future | not done after creation; a contextvar set in the first step is visible later and not to the caller; correct result |
| `current_task()` | in the first step it is the returned task; afterwards the outer task is back; also when created inside another task's step |
| `asyncio.timeout` in the first step (3.11+) | times out normally, no "not inside a task" |
| Cancel right after creation / a few iterations later / swallow the cancellation | the coroutine gets `CancelledError` at its `await`, the awaited future is cancelled; when swallowed, the task completes normally |
| Order of `sleep(0)` relative to other callbacks; order of resumption when `F` is already done / completes later | the recorded order equals the expectation |
| Raises after the first step | the task holds the exception |
| A task factory is set | the factory is not called |
| `get_stack()` | the top frame is the plugin coroutine |
| Unretrieved exceptions | no asyncio log records (captured from the `asyncio` logger) |

How it runs:

- every version: `_eager.start` runs every scenario and equals the expectation;
- 3.12+: native `asyncio.Task(..., eager_start=True)` runs every scenario and equals the same expectation;
- 3.12+: `_eager.start` runs as well (guards against a wrong expectation). If a future version changes `_current_tasks`, only the emulation is skipped on that version, with the reason written down (Q7.8); production code does not use the emulation on 3.12+.

3.10 has no `asyncio.timeout`; that scenario is skipped on 3.10 with the reason stated.

The prototype (section 12), 17 scenarios: on 3.10.16 and 3.11.11 the emulated records equal the native records from 3.12+; on 3.12.2, 3.13.6 and 3.14.0a5 emulation and native agree in every scenario.

### 6.2 Async methods through the peer: `python/rutis/tests/test_peer.py` (additions)

With the existing scripted peer on a socketpair:

1. **Awaiting an async method that never suspends on a synchronous chain** (risk P10 / the regression of this issue): with `_waiting=["node:1"]`, receive `invoke rust:1` (`path` `["node:1"]`) for an `async def` method that does not suspend; then receive an `await` of the returned reference (same chain). Expected: `return`, not `SyncWaitCycle`. On 3.10 / 3.11 this fails if the code falls back to lazy start.
2. **The prefix runs before the reply**: the method's first step records a marker; when the `return` frame is read, the marker exists.
3. **A synchronous callback in the first step**: the method's first step calls the far end with `peer.call(...)`; after the far end replies, the method continues; the frames are "callback request → callback result → the method's `return`".
4. **Cancellation**: the method suspends on a gated future; the far end sends `cancel`; the coroutine gets `CancelledError`, and `await` gets `throw` `CancelledError`. A second case: `invoke` and `cancel` delivered in the same batch of frames (before the task's first `__step`).
5. **Cancellation during the first step** (3.5.1): the method's first step calls the far end synchronously; before replying, the far end sends `cancel` for this call; the coroutine gets `CancelledError` at its first suspension point.
6. **Exceptions**: raised in the first step → the `invoke` reply is a `return` of a completed future reference, and awaiting it gets `throw` with the exception's class name; raised after suspending → the same.
7. **Call chain**: calls the method makes after its `await` carry this call's id in `path`.

### 6.3 websocket: `python/rutis/tests/test_websocket.py` (additions)

1. `serve_forever` replaced by one that returns at once (a failed `accept()`): `accept()` in another thread raises `OSError` within 10 s with "stopped" in the message; `close()` raises the same error; a later `dial` is refused.
2. `accept()` after `close()` raises `ConnectionError` at once.
3. A real `EMFILE` (Linux, macOS; skipped on Windows with the reason stated): a child process lowers `RLIMIT_NOFILE`, fills its descriptors and listens; the parent connects; the child's `accept()` raises, the process exits non-zero, and stderr names the cause.
4. `__main__.serve`: with a replaced `websocket.listen` whose `accept()` fails, `serve()` raises, and the current session's `end()` has run.

### 6.4 Rust side

The necessary condition, with no new Rust tests: the existing loader / bridge tests run on 3.10 (the CI `rust` job, section 7). Locally on 3.10, `multilang`, `instance_runtimes` and `python_rows` fail before the change and pass after it, so they already cover async methods.

Proposed: one bridge test that pins the semantics of 2.2 independently of the Python version: `rutis-bridge/tests/python_runtime.rs` `an_async_python_method_that_never_suspends_is_ready_inside_a_sync_chain`: Rust calls a non-suspending Python `async def` method on a synchronous call's chain and awaits the result; it gets the result, not `SyncWaitCycle`.

### 6.5 Install smoke

The PyPI install smoke of #193 (E2E S9) uses 3.10: `uv venv -p 3.10`, install `rutis-host`, and a Python plugin generated by `uvx rutis-host new` runs. This is one of #196's acceptance criteria and runs in #193's job.

## 7. CI (this PR changes the Linux Python version; #256 the rest)

Following #204's scope note: no new PR jobs; versions are replaced.

| Job | Machine | Now | Proposed |
| --- | --- | --- | --- |
| `rust` | Linux | 3.12 | **3.10** (minimum; every loader / bridge / host Rust test goes through the emulation) |
| `js-py` | Linux | 3.12 | **3.10** (Python unit tests) |
| `network-macos` | macOS | 3.12 | **latest** (native path; Python unit tests and the platform-dependent Rust tests) |
| `runtimes-windows` | Windows | 3.12 | latest |
| `runtimes-go`, `runtimes-bun` | Linux / macOS | 3.12 | 3.10 on Linux, latest on macOS (Python is only a dependency there and follows the platform) |

- websockets: `websockets==15.*` on Linux, latest on macOS / Windows (#204's table).
- How "latest" is written is `'3.x'`, changed by #256.
- No extra 3.11 or 3.12 runs: Q10.2 only requires the minimum and the latest, as in #204's rule. 3.11 takes the same emulation path as 3.10; the tests of 6.1 were run on 3.11 and 3.12 on a development machine (section 12).
- `docs/ci.md` / `ci.en.md` are updated together with `ci.yml` by the CI-side PR.

## 8. Documentation

Chinese and English together:

| File | Change |
| --- | --- |
| `docs/guide/python-plugin.md` / `.en.md` line 3 | "Python 3.10 or later is required" |
| `docs/guide/README.md` / `.en.md` line 33 | same |
| `docs/design-philosophy.md` / `.en.md` §6 | "Python 3.10+" |
| `docs/design-multilang-runtimes-2026-10-03.md` / `.en.md` line 148 | "Python 3.10+ (on 3.10 / 3.11 rutis implements eager start itself, see this document)" |
| `docs/design-developer-packages-2026-10-06.md` / `.en.md` line 278 | 3.10 or later |
| `docs/guide/plugin-api.md` / `.en.md` "Synchronous calls and reentrancy" | add: an `async` method runs up to its first `await` before the reply; an `async` method that never suspends can be awaited on a synchronous chain; the same on every Python version |
| `python/rutis/README.md` | "Python 3.10 or later" |
| doc comment in `crates/rutis-bridge/src/runtime/local.rs:62` | 3.10 |
| next release notes (0.9) | the Python minimum drops to 3.10 |

Not changed: past release notes (`docs/releases/0.7.0*`, `0.8.0*`), migration guides, `docs/plan/analysis/multilang/python*.md` (a survey record that keeps its conclusions of the time), `docs/quality-status.md` (updated at the end of the step, when Q10.2 / risk MX get "Python 3.10 and latest verified").

## 9. Risks

| Risk | Consequence | Handling |
| --- | --- | --- |
| An undiscovered difference between emulation and native | timing of some plugin differs on 3.10 / 3.11 from 3.12 | the fixed expectations of 6.1 are asserted on all three version ranges; every Rust test runs on 3.10 |
| Use of the private `asyncio.tasks._current_tasks` | only on 3.10 / 3.11, which get no more feature changes | not used on 3.12+; if tests find it broken on 3.12+, only the emulation test is skipped there |
| One extra task per call when the first step finishes | about 4–10 µs per call | measured in 3.10; frequent calls are already advised to stay in one runtime or move to Rust |
| 3.10 is end of life | security fixes only come from distributions (Ubuntu 22.04 and others) | the target users' environments decide; 3.10 stays verified in CI, and once it is no longer verified it is removed from the support statement (Q10.3) |
| websockets swallows `accept()`'s errno | the message can only name the likely cause | the message says so; test 6.3 item 3 confirms a real `EMFILE` reaches this path |
| Ending the process when listening fails | the current session ends with it | section 13 item 1; the lease is cleaned up first and the exit code is non-zero |

## 10. Phases (all in this PR)

1. **Eager start**: `_eager.py`, the `_task` change, `test_eager.py`, the tests of 6.2; all pass locally on 3.10 / 3.11 / 3.12+.
2. **Version statements**: the three `requires-python`; the docs of section 8.
3. **websocket and `__main__`**: the changes of section 5 and the tests of 6.3.
4. **CI**: this PR switches the Linux jobs to Python 3.10 in the same PR as the `requires-python` change, so that Q10.4 (verify before declaring) holds; the macOS and Windows versions are changed by #256.

## 11. Acceptance (all automatable)

1. `python3.10 -m unittest discover -s tests`, `python3.11 …` and `python3.12+ …` (in `python/rutis`) all pass, and every scenario of `test_eager.py` equals its expectation on every version (3.10 skips the `asyncio.timeout` scenario for the stated reason).
2. The CI `rust` job passes on Python 3.10 with websockets 15, including all tests of `rutis-loader --features node,python,go,peer` and `rutis-bridge --all-features` and the loopback reruns.
3. The CI `network-macos` job passes on the latest Python.
4. `grep -rn "3\.12" python crates/rutis-host/pyproject.toml crates/rutis-host/templates docs/guide` only finds lines about native eager start.
5. The websocket tests of 6.3: after a listener failure `accept()` raises within 10 s; `python -m rutis listen:…` exits non-zero.
6. #193's S9 PyPI install smoke passes on 3.10.

## 12. Local verification record (2026-10-11, macOS arm64)

Verified:

- The prototype `_eager.py` with 17 scenarios (the table in 6.1): on 3.10.16 and 3.11.11 the emulation matches the expectations; on 3.12.2, 3.13.6 and 3.14.0a5 emulation and native agree in every scenario.
- `type(asyncio.tasks._current_tasks) is dict` and `asyncio.Task is _asyncio.Task` (the C implementation): true on 3.10–3.14.
- 3.10.16 + websockets 15.0.1: the 41 `python/rutis` unit tests pass (code unchanged).
- 3.10.16, code unchanged: `multilang` (4), `instance_runtimes` (2; a third test that does not start Python passes) and `python_rows` (1) fail; with the prototype wired into `_task`, they all pass.
- 3.10.16 + websockets 15.0.1, prototype wired into `_task` (a temporary, uncommitted change): `cargo test -p rutis-loader --features node,python,peer` and `cargo test -p rutis-bridge --all-features` pass completely (including `leases`, `python_runtime`, `runtime_conformance`, `session_matrix`, `rpc_callbacks`, `cancellation` and the WebSocket tests), with no failures. Loader tests for Go rows were not run (no `go` feature).
- websockets 15.0.1 source: `serve_forever()` returns when `accept()` raises `OSError`.
- Cost: section 3.10.

Not verified:

- the above on Linux (CI will);
- Windows;
- 3.15 (not available locally);
- what `serve_forever()` of websockets 16 / 17 does when `accept()` fails (to be read from the source during implementation and added to the test matrix of 6.3);
- the "cancel during the first step is ignored" finding in 3.5.1 comes from reading the code only; the implementation reproduces it with a test first.

## 13. Maintainer decisions (2026-10-11)

- When listening fails, the current session ends at once and the process exits non-zero after cleaning up the lease, leaving the restart to the process manager (section 5).
- The `cancel` that arrives during the first step and is ignored today is fixed in this PR (3.5.1).
- 3.12+ keeps native eager start.
- This PR switches the Linux Python in `ci.yml` to 3.10 itself (an exception to "`ci.yml` is changed only by #203 / #204"); macOS and Windows use the latest, written `3.x`, changed by #256.
- No task names, no runtime version check, no extra 3.11 or 3.12 runs in CI.
