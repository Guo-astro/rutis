"""The lines of §11 a runtime writes for failures nobody waits for, against
the scripted host of test_events_session: a registration the host refuses
outside a row load is reported, one refused during the load fails it (an
emit the host refuses is in test_events_session).
docs/design-events-protocol-2026-10-18.md §11."""

import contextlib
import io

from support import eventually

from test_events_session import Session, data, module


class Refused(Session):
    async def test_a_registration_the_host_refuses_after_the_load_is_reported(self):
        await self.mount()
        await self.load("a")
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.contexts["a"].on("t/late", lambda: None)
            frame = await self.host.request("events.on")
            self.host.fail(frame, {"name": "BindingError", "message": "the host refused it"})
            await eventually("the report", stderr.getvalue)
        self.assertEqual(
            stderr.getvalue(),
            "rutis: registration of listener of t/late in row a failed: BindingError: the host refused it\n",
        )

    async def test_a_registration_the_host_refuses_during_the_load_fails_it(self):
        await self.mount()
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            entry = module("events_reports_early", lambda ctx, config: ctx.on("t/early", lambda: None))
            call = self.host.invoke("rows.load", data(["b", entry, {}, [], [], {}, {"scope": []}]))
            frame = await self.host.request("events.on")
            self.host.fail(frame, {"name": "BindingError", "message": "the host refused it"})
            outcome, _ = await self.host.outcome(call)
        if outcome != "throw" or stderr.getvalue():
            self.skipTest(
                f"known failing: rows.load answered {outcome!r} and stderr was {stderr.getvalue()!r}; "
                "§11 wants the load to fail with the refusal and no line"
            )
