import asyncio, os, socket, subprocess, sys, time, types
sys.path.insert(0, "/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37/python/rutis")
from rutis import define_plugin
from rutis.peer import Peer
from rutis.runner import Runtime

HOST = "/Users/eric8810/Code/rutis/.claude/worktrees/agent-aad5ddc8a34de2a37/target/debug/rutis-host"
ready = None


def drv(ctx, config):
    ready.set_result(ctx)


sys.modules["rutis-test"] = types.SimpleNamespace(plugin=define_plugin(drv, inject=["rutis-host/test"]), __name__="rutis-test")


def weather(ctx, config):
    ctx.provide("weather", types.SimpleNamespace(today=lambda: f"sunny in {config['city']}"))


sys.modules["weather"] = types.SimpleNamespace(plugin=define_plugin(weather, config={"type": "object", "properties": {"city": {"type": "string", "default": "Oslo"}}}), __name__="weather")


async def main():
    global ready
    t0 = time.time()
    ready = asyncio.get_running_loop().create_future()
    ours, theirs = socket.socketpair()
    proc = subprocess.Popen([HOST, "test", "rutis-test"], stdin=theirs, stdout=theirs)
    theirs.close()
    rt = Runtime()
    peer = Peer(ours, rt.dispatch, settled=None, endpoint={"local": "python", "declare": ["runtime"]})
    rt.peer = peer
    rt.traced(peer)
    peer.start()
    await peer.ready
    print("ready", time.time() - t0)
    ctx = await ready
    t1 = time.time()
    h = ctx.use("rutis-host/test")
    id = await h.load("weather", None)
    t2 = time.time()
    print(id, rt.services)
    await h.unload(id)
    print("times", t1 - t0, t2 - t1, time.time() - t2)
    peer.close()
    ours.shutdown(socket.SHUT_RDWR)
    ours.close()
    print(proc.wait(timeout=5), time.time() - t0)


asyncio.run(main())
