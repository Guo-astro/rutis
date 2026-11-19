"""A real cross-language call for `chunked` (#1363): this Python session as
the host and the Node runtime's conformance fixture as the far end. A
parameter and a result over 16 MiB cross whole, and the method runs once."""

import asyncio
import os
import shutil
import socket
import subprocess
import tempfile
import unittest
from pathlib import Path

import background

from rutis.peer import Peer

ROOT = Path(__file__).resolve().parents[3]
FIXTURE = ROOT / "node" / "rutis-runtime" / "test" / "fixtures" / "conformance-session.mjs"
NODE_MODULES = ROOT / "node" / "rutis-runtime" / "node_modules"
NODE = shutil.which("node")
TIMEOUT = 30


@unittest.skipUnless(
    hasattr(socket, "AF_UNIX") and NODE and FIXTURE.exists() and NODE_MODULES.is_dir(),
    "the Node runtime fixture (Unix sockets, node and node/rutis-runtime/node_modules) is not here",
)
class NodeCrossLanguage(background.IsolatedAsyncioTestCase):
    async def start(self, directory):
        path = os.path.join(directory, "node.sock")
        server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        server.bind(path)
        server.listen(1)
        server.settimeout(TIMEOUT)
        self.addCleanup(server.close)
        process = subprocess.Popen(
            [NODE, str(FIXTURE), path],
            cwd=ROOT / "node" / "rutis-runtime",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self.addCleanup(self.stop, process)
        connection, _ = await asyncio.to_thread(server.accept)
        connection.settimeout(TIMEOUT)
        self.addCleanup(connection.close)
        peer = Peer(connection, lambda target, method, args: None, endpoint={"local": "main", "expected": "node"})
        self.addCleanup(peer.close)
        peer.start()
        await asyncio.wait_for(peer.ready, TIMEOUT)
        return peer, process

    def stop(self, process):
        if process.poll() is None:
            process.kill()
        process.wait(timeout=TIMEOUT)
        process.stdout.close()
        process.stderr.close()

    async def test_a_value_over_16_mib_crosses_whole_once(self):
        with tempfile.TemporaryDirectory() as directory:
            peer, process = await self.start(directory)
            big = "x" * (17 * 1024 * 1024)
            tally = peer.call("conformance", "tally", [big])
            self.assertEqual(tally, {"calls": 1, "size": len(big)})
            value = peer.call("conformance", "big", [len(big)])
            self.assertEqual(len(value), len(big))
            self.assertEqual(peer.call("conformance", "echo", ["after"]), "after")
            self.assertIsNone(peer.closed_error)
            peer.close()
            peer._channel.close()
            code = await asyncio.to_thread(process.wait, TIMEOUT)
            stderr = process.stderr.read()
            self.assertEqual(code, 0, stderr)


if __name__ == "__main__":
    unittest.main()
