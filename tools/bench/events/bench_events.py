"""The Python row of the event benchmark (run.sh): the emitter and its
listeners in one plugin, so in one runtime. Only the public plugin API
(`ctx.on`, `ctx.emit`, `ctx.serial`, `ctx.bail`): the same file measures
local dispatch and dispatch through the host.

Config: { out, iterations, warmup }. Writes { mode: { median_us, p99_us } }
as JSON to `out` when done.
"""

import asyncio
import json
import sys
import time
import traceback

from rutis import define_plugin


def summary(samples):
    samples.sort()
    at = lambda q: samples[min(len(samples) - 1, int(q * len(samples)))]
    return {"median_us": at(0.5), "p99_us": at(0.99), "samples": len(samples)}


def apply(ctx, config):
    out = config["out"]
    iterations = config.get("iterations", 10_000)
    warmup = config.get("warmup", 1_000)
    loop = asyncio.get_running_loop()
    waiting = {}

    # emit: the time from `emit` until the listener has run.
    def on_emit(i):
        waiting.pop("future").set_result(i)

    ctx.on("bench/emit", on_emit)
    ctx.on("bench/serial", lambda i: i)
    ctx.on("bench/bail", lambda i: i)

    async def emit(i):
        future = waiting["future"] = loop.create_future()
        ctx.emit("bench/emit", i)
        return await future

    async def serial(i):
        return await ctx.serial("bench/serial", i)

    async def bail(i):
        return ctx.bail("bench/bail", i)

    modes = {"emit": emit, "serial": serial, "bail": bail}

    async def run():
        report = {"runtime": "python", "python": sys.version.split()[0], "iterations": iterations, "warmup": warmup}
        for mode, once in modes.items():
            for i in range(warmup):
                await once(i)
            samples = [0.0] * iterations
            for i in range(iterations):
                start = time.perf_counter_ns()
                value = await once(i)
                samples[i] = (time.perf_counter_ns() - start) / 1000
                if value != i:
                    raise RuntimeError(f"{mode} returned {value!r}, expected {i}")
            report[mode] = summary(samples)
        with open(out, "w") as file:
            json.dump(report, file, indent=2)

    async def guarded():
        try:
            await run()
        except Exception:
            with open(out, "w") as file:
                json.dump({"error": traceback.format_exc()}, file)

    # After apply, so that the row is running when the measurement starts.
    task = loop.create_task(guarded())
    ctx.effect(task.cancel)


plugin = define_plugin(apply)
