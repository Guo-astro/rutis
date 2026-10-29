import pathlib

ROOT = pathlib.Path("/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37")


def edit(path, pairs):
    p = ROOT / path
    s = p.read_text()
    for old, new in pairs:
        assert s.count(old) == 1, (path, old[:80], s.count(old))
        s = s.replace(old, new)
    p.write_text(s)


edit("docs/guide/plugin-api.md", [
    ('''                self.assertEqual(await t.service("greeter").hello("Bo", "en"), "Hello, Bo!")
                self.assertEqual(said, ["Ada", "Bo"])
                self.assertEqual(await t.serial("greeting/pick", "sv"), "Hej")  # 测试自己也能发出
''', '''                self.assertEqual(await t.service("greeter").hello("Bo", "en"), "Hello, Bo!")
                self.assertEqual(await t.serial("greeting/pick", "sv"), "Hej")  # 测试自己也能发出
                self.assertEqual(said, ["Ada", "Bo"])    # emit 经宿主，在它之后的 serial 回来时已送到
'''),
    ('''- 作用域：`load(plugin, bus=bus, scope="t1/s1")` 把插件加载到一个实例里（`Bus(scopes={"t1": "tenant"})` 给出组名）；监听器收到这个 root 总线上的所有同名事件。
''', ''),
    ('''测试工具（`@arcships/rutis/testing`、`rutis.testing`、Go 的 `rutistest`）和真实运行时对同一份期望文件比对''',
     '''测试工具（`@arcships/rutis/testing`、Go 的 `rutistest`）和真实运行时对同一份期望文件比对'''),
    ('''| Python：跨过边界的异常是 `RemoteError`，它的 `__cause__` 是原来的异常 | 方便调试；真实运行时的 `RemoteError` 没有 `__cause__` |
''', ''),
    ('''TypeScript `host({ deadline })` / `load(…, { deadline, runtime })`（毫秒），Python `load(…, deadline=, runtime=)`（秒），Go `rutistest.RuntimeName` 等选项''',
     '''TypeScript `host({ deadline })` / `load(…, { deadline, runtime })`（毫秒），Go `rutistest.RuntimeName` 等选项'''),
    ('''作用域（TypeScript `host()` 的 `scope`、Python `Bus` 与 `scope=`、Go `rutistest.InScope`）''',
     '''作用域（TypeScript `host()` 的 `scope`、Go `rutistest.InScope`）'''),
    ('''TypeScript、Bun 和 Python 的测试工具在测试的进程里运行插件，默认是严格模式：值按跨进程的规则传递，数据被复制（Python 的 dataclass 变成 dict），函数和带方法的对象按引用传递（交出的是代理：只有方法，每次调用都是异步的），声明为 `sync` 的方法不能返回 Promise / 协程。只在同一进程里才成立的写法，在这里就会失败。`strict: false`（Python `strict=False`）关掉它，这时值不经过[值怎样传递](#值怎样传递)的规则。''',
     '''TypeScript 和 Bun 的测试工具在测试的进程里运行插件，默认是严格模式：值按跨进程的规则传递，数据被复制，函数和带方法的对象按引用传递（交出的是代理：只有方法，每次调用都是异步的），声明为 `sync` 的方法不能返回 Promise。只在同一进程里才成立的写法，在这里就会失败。`strict: false` 关掉它，这时值不经过[值怎样传递](#值怎样传递)的规则。'''),
    ('''| `load(plugin, *, …)` 的参数 | 作用 |
| --- | --- |
| `config` | 插件的配置；有 `config` 声明时补默认值并校验 |
| `services` | 插件 `inject` 的服务，按名字给 |
| `strict` | 值按跨进程的规则传递（默认 `True`） |
| `bus` | 与同一个 `Bus` 上的插件共用事件（默认自己一个） |
| `scope` | 加载到哪个作用域路径（`t1/s1`，默认根） |
| `row` | 在总线上的行名（默认插件声明的名字，否则模块名） |
| `runtime` | 监听器描述里的 `runtime`（默认 `"python"`） |
| `deadline` | 卸载时等正在运行的监听器多少秒（默认 `RUTIS_SHUTDOWN_TIMEOUT`，10） |
''', '''`load` 起一个 `rutis-host test`（`RUTIS_HOST` 指定的文件，没有时用 `PATH` 上的 `rutis-host`；插件项目的开发依赖 `rutis-host` 装的就是它），把测试进程里的 Python 运行时接上去，在测试的事件循环里运行。插件是这个宿主的一行，和在别的宿主上一样运行：配置由宿主的 loader 校验并补上默认值，事件全部经宿主，卸载也由宿主做。测试给的服务（`services`）和测试自己的监听器（`t.on`）属于同一个运行时里测试的插件 `tester`。

| `load(plugin, *, …)` 的参数 | 作用 |
| --- | --- |
| `config` | 插件的配置；宿主的 loader 按插件的 `config` 声明补默认值并校验，不合格时 `load` 以这一行的状态行失败（`weather: config: city is required`） |
| `services` | 插件 `inject` 的服务，按名字给 |
| `bus` | 与同一个 `Bus` 上的插件共用一个宿主（默认自己一个） |
| `row` | 行名（默认插件声明的名字，否则模块名）；已有同名的行时宿主加 `-2`、`-3`…… |
'''),
    ('''| `t.on(name, listener, *, prepend=False, global_=False)` | 像宿主一样在根作用域监听：插件发出的、测试自己发出的都送到；严格模式下参数像从另一个进程收到的；返回撤销函数，不撤销时随插件卸载撤掉 |
| `t.emit`、`t.parallel`、`t.serial`、`t.waterfall`、`t.bail` | 从插件所在的行和作用域发出事件 |''',
     '''| `t.on(name, listener, *, prepend=False, global_=False)` | 以测试的插件监听：插件发出的、测试自己发出的都送到；参数经宿主传来；返回撤销函数，不撤销时随插件卸载撤掉 |
| `t.emit`、`t.parallel`、`t.serial`、`t.waterfall`、`t.bail` | 从插件所在的行发出事件，经宿主 |'''),
    ('''| `t.reported` | 没人接收的失败（`emit` 里出错的监听器、过滤函数） |
| `await t.update(config)` | 像宿主的 `rows.update` 那样换配置：卸载后用新配置再运行 |
| `await t.unload()` | 像运行时那样卸载：撤下监听器和服务，取消并等待正在运行的监听器，再按倒序运行清理（退出 `async with` 时自动做） |

`Bus(scopes=...)`：一条测试用的总线，同一个 `bus` 上的插件互相听得到；`scopes` 给作用域路径起组名。它的 `reported`、`listeners(name)`（过滤函数看到的描述）、`emitter(row=..., strict=...)`（以某一行的身份发出）。`Reported`：没人接收的一次失败，字段 `kind`（`"listener"` 或 `"filter"`）、`name`、`row`、`error`。`PluginTestError`：插件做了宿主会拒绝或跨进程会出错的事，是 `AssertionError` 的子类。''',
     '''| `t.reported` | 这个进程里没人接收的失败（`emit` 里出错的监听器） |
| `await t.update(config)` | 由宿主换配置：卸载后用新配置再运行；配置不合格时以宿主的话失败，这一行保持原配置 |
| `await t.unload()` | 由宿主卸载：撤下监听器和服务，取消并等待正在运行的监听器，再按倒序运行清理（退出 `async with` 时自动做） |

`Bus()`：一个测试用的宿主，同一个 `bus` 上的插件在同一个宿主上，互相听得到；第一个 `load` 时起，最后一个结束时停。它的 `reported`、`emitter()`（以测试的插件发出）。`Reported`：没人接收的一次失败，字段 `kind`（`"listener"` 或 `"emit"`）、`name`、`row`、`error`。`PluginTestError`：宿主拒绝装载或换配置（文字是宿主的话），插件没有提供要的服务，或起不了 `rutis-host`；是 `AssertionError` 的子类。'''),
])

edit("docs/guide/plugin-api.en.md", [
    ('''                self.assertEqual(await t.service("greeter").hello("Bo", "en"), "Hello, Bo!")
                self.assertEqual(said, ["Ada", "Bo"])
                self.assertEqual(await t.serial("greeting/pick", "sv"), "Hej")  # the test can emit too
''', '''                self.assertEqual(await t.service("greeter").hello("Bo", "en"), "Hello, Bo!")
                self.assertEqual(await t.serial("greeting/pick", "sv"), "Hej")  # the test can emit too
                self.assertEqual(said, ["Ada", "Bo"])    # emits go through the host: delivered once the serial after them is back
'''),
    ('''- Scopes: `load(plugin, bus=bus, scope="t1/s1")` loads a plugin into an instance (`Bus(scopes={"t1": "tenant"})` names the groups); a listener receives every event of the name on the root bus.
''', ''),
    ('''The test tools (`@arcships/rutis/testing`, `rutis.testing`, Go's `rutistest`) and the real runtimes are checked against the same expectations file''',
     '''The test tools (`@arcships/rutis/testing`, Go's `rutistest`) and the real runtimes are checked against the same expectations file'''),
    ('''| Python: an exception that crosses is a `RemoteError` whose `__cause__` is the original exception | For debugging; a real runtime's `RemoteError` has no `__cause__` |
''', ''),
    ('''TypeScript `host({ deadline })` / `load(…, { deadline, runtime })` in ms, Python `load(…, deadline=, runtime=)` in seconds, Go `rutistest.RuntimeName` and other options''',
     '''TypeScript `host({ deadline })` / `load(…, { deadline, runtime })` in ms, Go `rutistest.RuntimeName` and other options'''),
    ('''scopes (TypeScript `host()` with `scope`, Python `Bus` and `scope=`, Go `rutistest.InScope`)''',
     '''scopes (TypeScript `host()` with `scope`, Go `rutistest.InScope`)'''),
    ('''The TypeScript, Bun and Python test tools run the plugin in the test's process, in strict mode by default: values follow cross-process rules, data is copied (a Python dataclass becomes a dict), functions and objects with methods are passed by reference (what is handed over is a proxy: methods only, every call asynchronous), and a method declared `sync` cannot return a Promise / coroutine. Code that works only within one process fails here. `strict: false` (Python `strict=False`) turns it off, and values then skip the rules of [passing values](#passing-values).''',
     '''The TypeScript and Bun test tools run the plugin in the test's process, in strict mode by default: values follow cross-process rules, data is copied, functions and objects with methods are passed by reference (what is handed over is a proxy: methods only, every call asynchronous), and a method declared `sync` cannot return a Promise. Code that works only within one process fails here. `strict: false` turns it off, and values then skip the rules of [passing values](#passing-values).'''),
    ('''| Parameter of `load(plugin, *, …)` | What it does |
| --- | --- |
| `config` | The plugin's config; with a `config` declaration, defaults are filled in and it is validated |
| `services` | The services the plugin `inject`s, by name |
| `strict` | Values follow cross-process rules (default `True`) |
| `bus` | Share events with the plugins on the same `Bus` (default: one of its own) |
| `scope` | The scope path it is loaded in (`t1/s1`; default the root) |
| `row` | Its row name on the bus (default the plugin's declared name, else its module's) |
| `runtime` | What listener descriptors give as `runtime` (default `"python"`) |
| `deadline` | How many seconds unloading waits for running listeners (default `RUTIS_SHUTDOWN_TIMEOUT`, 10) |
''', '''`load` starts `rutis-host test` (the file `RUTIS_HOST` names, else `rutis-host` on `PATH`; the plugin project's dev dependency `rutis-host` installs it) and connects the Python runtime in the test's process to it, in the test's event loop. The plugin is a row of that host and runs as it does on any host: the host's loader checks its config and fills in the defaults, every event goes through the host, and the host unloads it. The services the test gives (`services`) and the test's own listeners (`t.on`) belong to the test's plugin `tester`, in the same runtime.

| Parameter of `load(plugin, *, …)` | What it does |
| --- | --- |
| `config` | The plugin's config; the host's loader fills in the defaults of the plugin's `config` declaration and checks it, and a config that does not match fails `load` with the row's state line (`weather: config: city is required`) |
| `services` | The services the plugin `inject`s, by name |
| `bus` | Share one host with the plugins on the same `Bus` (default: one of its own) |
| `row` | Its row's id (default the plugin's declared name, else its module's); the host adds `-2`, `-3`… to an id taken |
'''),
    ('''| `t.on(name, listener, *, prepend=False, global_=False)` | Listen as the host does, at the root scope: the plugin's emissions and the test's own reach it; in strict mode it gets the arguments as from another process; returns the function that stops it, and without it, it stops when the plugin unloads |
| `t.emit`, `t.parallel`, `t.serial`, `t.waterfall`, `t.bail` | Emit from the plugin's row and scope |''',
     '''| `t.on(name, listener, *, prepend=False, global_=False)` | Listen from the test's plugin: the plugin's emissions and the test's own reach it, its arguments sent by the host; returns the function that stops it, and without it, it stops when the plugin unloads |
| `t.emit`, `t.parallel`, `t.serial`, `t.waterfall`, `t.bail` | Emit from the plugin's row, through the host |'''),
    ('''| `t.reported` | The failures nobody received (listeners failing in an `emit`, filters) |
| `await t.update(config)` | Change the config as a host's `rows.update` does: unload, then run again with the new config |
| `await t.unload()` | Unload as the runtime does: withdraw listeners and services, cancel and wait for running listeners, then run the cleanups, latest first (done when the `async with` block exits) |

`Bus(scopes=...)`: a bus for tests; the plugins on the same `bus` hear each other, and `scopes` names the group of a scope path. It has `reported`, `listeners(name)` (the descriptors a filter sees) and `emitter(row=..., strict=...)` (emit as a given row). `Reported`: one failure nobody received, with the fields `kind` (`"listener"` or `"filter"`), `name`, `row` and `error`. `PluginTestError`: the plugin did something a host would refuse or that would fail across processes; a subclass of `AssertionError`.''',
     '''| `t.reported` | The failures nobody received in this process (listeners failing in an `emit`) |
| `await t.update(config)` | The host changes the config: unload, then run again with the new config; a config that does not match fails in the host's words, and the row keeps its config |
| `await t.unload()` | The host unloads it: withdraw listeners and services, cancel and wait for running listeners, then run the cleanups, latest first (done when the `async with` block exits) |

`Bus()`: a host for tests; the plugins on the same `bus` are on one host and hear each other; it starts with the first `load` and stops when the last one ends. It has `reported` and `emitter()` (emit as the test's plugin). `Reported`: one failure nobody received, with the fields `kind` (`"listener"` or `"emit"`), `name`, `row` and `error`. `PluginTestError`: the host refused to load the plugin or change its config (the text is the host's), the plugin does not provide the service asked for, or `rutis-host` cannot be started; a subclass of `AssertionError`.'''),
])
print("ok")
