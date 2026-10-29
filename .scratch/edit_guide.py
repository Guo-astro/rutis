import pathlib

ROOT = pathlib.Path("/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37")


def edit(path, pairs):
    p = ROOT / path
    s = p.read_text()
    for old, new in pairs:
        assert s.count(old) == 1, (path, old[:80], s.count(old))
        s = s.replace(old, new)
    p.write_text(s)


OLD_TEST = '''        async def go():
            async with load(plugin, config={"greeting": "Hi"}, services={"translate": FakeTranslate()}) as t:
                greeted = []
                t.on("greeter/greeted", greeted.append)
                self.assertEqual(await t.service("greeter").hello("Ada"), "Salut, Ada!")
                self.assertEqual(greeted, ["Ada"])
                self.assertEqual(await t.service("greeter").count(), 1)
'''
NEW_TEST_ZH = '''        async def go():
            greeted = []
            async with load(plugin, config={"greeting": "Hi"}, services={"translate": FakeTranslate()}) as t:
                t.on("greeter/greeted", greeted.append)
                self.assertEqual(await t.service("greeter").hello("Ada"), "Salut, Ada!")
                self.assertEqual(await t.service("greeter").count(), 1)
            # 事件经宿主送到；插件卸载之后，卸载之前发出的都已送到
            self.assertEqual(greeted, ["Ada"])
'''
NEW_TEST_EN = '''        async def go():
            greeted = []
            async with load(plugin, config={"greeting": "Hi"}, services={"translate": FakeTranslate()}) as t:
                t.on("greeter/greeted", greeted.append)
                self.assertEqual(await t.service("greeter").hello("Ada"), "Salut, Ada!")
                self.assertEqual(await t.service("greeter").count(), 1)
            # Events go through the host; once the plugin unloaded, what it emitted before has arrived
            self.assertEqual(greeted, ["Ada"])
'''

edit("docs/guide/python-plugin.md", [
    ("`rutis.testing` 的 `load` 不需要宿主：装上插件、给它配置和假的 `translate`，取它的服务来调用，监听它发出的事件；退出 `async with` 时卸载插件。",
     "`rutis.testing` 的 `load` 起一个测试用的宿主（`rutis-host test`，项目的开发依赖 `rutis-host` 装好了它），在测试的进程里用真实的 Python 运行时装上插件：给它配置和假的 `translate`，取它的服务来调用，监听它发出的事件；退出 `async with` 时卸载插件。"),
    (OLD_TEST, NEW_TEST_ZH),
    ("`load` 检查宿主会检查的事：配置按 Schema 补默认值并校验，不合格时抛出 `BindingError`（`code` 是 `bad-config`）；插件只用 `inject` 里写的服务；提供的服务带着 `provides` 里写的方法；值按跨进程的规则传递。",
     "检查都是宿主自己做的：配置由宿主的 loader 按 Schema 补默认值并校验，不合格时 `load` 抛出 `PluginTestError`，文字是这一行的状态行（`greeter: config: greeting should be a string, got 3`）；事件经宿主分发。"),
])
edit("docs/guide/python-plugin.en.md", [
    ("`load` from `rutis.testing` needs no host: it loads the plugin with a config and a fake `translate`, gives you its service to call and its events to listen to, and unloads the plugin when the `async with` ends.",
     "`load` from `rutis.testing` starts a host for the test (`rutis-host test`, installed by the project's dev dependency `rutis-host`) and loads the plugin on the real Python runtime, in the test's process: with a config and a fake `translate`, it gives you its service to call and its events to listen to, and unloads the plugin when the `async with` ends."),
    (OLD_TEST, NEW_TEST_EN),
])
print("ok")
