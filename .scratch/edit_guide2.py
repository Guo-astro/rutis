import pathlib

ROOT = pathlib.Path("/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37")


def edit(path, old, new):
    p = ROOT / path
    s = p.read_text()
    assert s.count(old) == 1, (path, old[:80])
    p.write_text(s.replace(old, new))


edit("docs/guide/python-plugin.md", '''        async def go():
            greeted = []
            async with load(plugin, config={"greeting": "Hi"}, services={"translate": FakeTranslate()}) as t:
                t.on("greeter/greeted", greeted.append)
                self.assertEqual(await t.service("greeter").hello("Ada"), "Salut, Ada!")
                self.assertEqual(await t.service("greeter").count(), 1)
            # 事件经宿主送到；插件卸载之后，卸载之前发出的都已送到
            self.assertEqual(greeted, ["Ada"])
''', '''        async def go():
            async with load(plugin, config={"greeting": "Hi"}, services={"translate": FakeTranslate()}) as t:
                greeted = []
                t.on("greeter/greeted", greeted.append)
                self.assertEqual(await t.service("greeter").hello("Ada"), "Salut, Ada!")
                # 测试也能发出；parallel 等全部监听器做完，之前的 emit 也已送到
                await t.parallel("greeter/greeted", "Bo")
                self.assertEqual(greeted, ["Ada", "Bo"])
                self.assertEqual(await t.service("greeter").count(), 2)
''')
edit("docs/guide/python-plugin.en.md", '''        async def go():
            greeted = []
            async with load(plugin, config={"greeting": "Hi"}, services={"translate": FakeTranslate()}) as t:
                t.on("greeter/greeted", greeted.append)
                self.assertEqual(await t.service("greeter").hello("Ada"), "Salut, Ada!")
                self.assertEqual(await t.service("greeter").count(), 1)
            # Events go through the host; once the plugin unloaded, what it emitted before has arrived
            self.assertEqual(greeted, ["Ada"])
''', '''        async def go():
            async with load(plugin, config={"greeting": "Hi"}, services={"translate": FakeTranslate()}) as t:
                greeted = []
                t.on("greeter/greeted", greeted.append)
                self.assertEqual(await t.service("greeter").hello("Ada"), "Salut, Ada!")
                # The test can emit too; parallel waits for every listener, by then the emit before it has arrived
                await t.parallel("greeter/greeted", "Bo")
                self.assertEqual(greeted, ["Ada", "Bo"])
                self.assertEqual(await t.service("greeter").count(), 2)
''')
print("ok")
