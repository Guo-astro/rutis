import pathlib

p = pathlib.Path("/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37/python/rutis/tests/test_events_session.py")
s = p.read_text()


def rep(old, new):
    global s
    assert s.count(old) == 1, old[:60]
    s = s.replace(old, new)


def cut(a, b):
    global s
    i = s.index(a)
    j = s.index(b, i)
    s = s[:i] + s[j:]


rep('''    async def test_a_name_listened_to_elsewhere_is_dispatched_by_the_host(self):
        # §5.2, §5.3: the request names the row, never a scope.
        await self.mount({"add": ["t/x"]})
        log = []
        await self.load("a", lambda ctx, config: ctx.on("t/x", lambda value: log.append(value)), registrations=1)
        ctx = self.contexts["a"]
        dispatch = asyncio.ensure_future(ctx.serial("t/x", 1))
        frame = await self.host.request("events.dispatch")
        self.assertEqual(frame["value"], [{"row": "a", "name": "t/x", "mode": "serial", "args": [1]}])
        self.assertEqual(log, [], "its own listener is called back by the host, not here")
        self.host.answer(frame, "R")
        self.assertEqual(await within("the dispatch", dispatch), "R")
        # Prefixes count too; removed names are dispatched here again.
        await self.elsewhere(remove=["t/x"], addPrefixes=["t/"])
        dispatch = asyncio.ensure_future(ctx.parallel("t/x", 2))
        frame = await self.host.request("events.dispatch")
        self.host.answer(frame)
        self.assertIsNone(await within("the dispatch", dispatch))
        await self.elsewhere(removePrefixes=["t/"])
        self.assertIsNone(await ctx.serial("t/x", 3))
        self.assertEqual(log, [3])
''', '''    async def test_every_emission_is_dispatched_by_the_host(self):
        # §5.2, §5.3: the request names the row, never a scope; the row's
        # own listener is called back by the host, also when no listener
        # of the name is elsewhere.
        await self.mount()
        log = []
        await self.load("a", lambda ctx, config: ctx.on("t/x", lambda value: log.append(value)), registrations=1)
        ctx = self.contexts["a"]
        dispatch = asyncio.ensure_future(ctx.serial("t/x", 1))
        frame = await self.host.request("events.dispatch")
        self.assertEqual(frame["value"], [{"row": "a", "name": "t/x", "mode": "serial", "args": [1]}])
        self.assertEqual(log, [], "its own listener is called back by the host, not here")
        self.host.answer(frame, "R")
        self.assertEqual(await within("the dispatch", dispatch), "R")
''')
rep('''    async def test_emits_through_the_host_keep_their_order(self):
        # §5.5: while an emit of a name through the host is unanswered, the
        # next ones follow it there, even once nothing is elsewhere.
        await self.mount({"add": ["t/e"]})
        log = []
        await self.load("a", lambda ctx, config: ctx.on("t/e", lambda n: log.append(n)), registrations=1)
        ctx = self.contexts["a"]
        self.assertIsNone(ctx.emit("t/e", 1))
        first = await self.host.request("events.dispatch")
        await self.elsewhere(remove=["t/e"])
        ctx.emit("t/e", 2)
        second = await self.host.request("events.dispatch")
        self.assertEqual([first["value"][0]["args"], second["value"][0]["args"]], [[1], [2]])
        self.assertEqual(log, [])
        self.host.answer(first)
        self.host.answer(second)
        await eventually("both answered", lambda: not self.host.runtime.unanswered)
        ctx.emit("t/e", 3)
        self.assertEqual(log, [3], "the fast path again")
''', '''    async def test_emits_through_the_host_keep_their_order(self):
        # §5.5: emits of a name reach the host in the order they were made.
        await self.mount()
        log = []
        await self.load("a", lambda ctx, config: ctx.on("t/e", lambda n: log.append(n)), registrations=1)
        ctx = self.contexts["a"]
        self.assertIsNone(ctx.emit("t/e", 1))
        ctx.emit("t/e", 2)
        first = await self.host.request("events.dispatch")
        second = await self.host.request("events.dispatch")
        self.assertEqual([first["value"][0]["args"], second["value"][0]["args"]], [[1], [2]])
        self.assertEqual(log, [])
        self.host.answer(first)
        self.host.answer(second)
''')
cut("    async def test_an_emit_reply_is_taken_before_the_next_frame(self):",
    "    async def test_a_0_13_hosts_future_answer_counts_once_it_settles(self):")
rep('''    async def test_a_0_13_hosts_future_answer_counts_once_it_settles(self):
        # A 0.13 host answers a queued emit with a future: the emit stays
        # unanswered until it settles.
        await self.mount({"add": ["t/e"]})
        await self.load("a")
        runtime = self.host.runtime
        self.contexts["a"].emit("t/e", 1)
        frame = await self.host.request("events.dispatch")
        self.host.send({"op": "return", "id": frame["id"], "value": {"type": "reference", "value": {"id": 1, "kind": "future", "home": False, "origin": []}}})
        waited = await self.host.next(lambda f: f["op"] == "await")
        self.assertEqual(waited["reference"], 1)
        self.assertEqual(runtime.unanswered, {"t/e": 1})
        self.host.send({"op": "return", "id": waited["id"], "value": {"type": "undefined"}})
        await eventually("answered once settled", lambda: not runtime.unanswered)
''', '''    async def test_a_future_answer_is_awaited_and_its_failure_reported(self):
        # A host answering an emit with a future: its settlement is the
        # emit's outcome.
        await self.mount()
        await self.load("a")
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.contexts["a"].emit("t/e", 1)
            frame = await self.host.request("events.dispatch")
            self.host.send({"op": "return", "id": frame["id"], "value": {"type": "reference", "value": {"id": 1, "kind": "future", "home": False, "origin": []}}})
            waited = await self.host.next(lambda f: f["op"] == "await")
            self.assertEqual(waited["reference"], 1)
            self.host.fail(waited, {"name": "BindingError", "message": "gone"})
            await eventually("the report", stderr.getvalue)
        self.assertEqual(stderr.getvalue(), "rutis: emit of t/e from row a failed: BindingError: gone\\n")
''')
p.write_text(s)
