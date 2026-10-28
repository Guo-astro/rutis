"""A local channel speaks the session protocol as its `--id` (#811 R3): the
runtime greets with a hello naming that id and declaring `runtime`, takes
the host as its `--peer`, and the session runs until the host ends it; a
local channel without `--id` is refused. A real runtime process
(`python -m rutis`) on a Unix socket."""

import json
import os
import socket
import subprocess
import sys
import tempfile
import unittest

import rutis
from rutis.peer import ENDPOINT_PROTOCOL

TIMEOUT = 30


class LocalEndpoint(unittest.TestCase):
    @unittest.skipUnless(hasattr(socket, "AF_UNIX"), "Unix sockets")
    def test_a_local_channel_greets_as_its_id(self):
        package = os.path.dirname(os.path.dirname(os.path.abspath(rutis.__file__)))
        with tempfile.TemporaryDirectory() as directory:
            path = os.path.join(directory, "host.sock")
            server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            server.bind(path)
            server.listen(1)
            server.settimeout(TIMEOUT)
            child = subprocess.Popen(
                [sys.executable, "-m", "rutis", path, "--id", "weather", "--peer", "host", directory],
                stderr=subprocess.PIPE,
                text=True,
                env={**os.environ, "PYTHONPATH": package},
            )
            try:
                connection, _ = server.accept()
                connection.settimeout(TIMEOUT)
                hello = {
                    "op": "hello",
                    "version": ENDPOINT_PROTOCOL,
                    "endpoint": "host",
                    "implementation": {"name": "rutis-host", "version": "0.19.0"},
                    "capabilities": ["signals", "sync-wait", "events-v2"],
                }
                connection.sendall(json.dumps(hello).encode() + b"\n")
                received = b""
                while b"\n" not in received:
                    chunk = connection.recv(65536)
                    self.assertTrue(chunk, "the runtime closed before greeting")
                    received += chunk
                frame = json.loads(received.split(b"\n", 1)[0])
                self.assertEqual(frame["version"], ENDPOINT_PROTOCOL)
                self.assertEqual(frame["endpoint"], "weather")
                self.assertIn("runtime", frame["capabilities"])
                # The host ends the session; the runtime stops normally.
                connection.close()
                code = child.wait(timeout=TIMEOUT)
                stderr = child.stderr.read()
                self.assertNotIn("refused", stderr)
                self.assertEqual(code, 0, stderr)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()
                child.stderr.close()
                server.close()

    def test_a_local_channel_without_id_is_refused(self):
        package = os.path.dirname(os.path.dirname(os.path.abspath(rutis.__file__)))
        with tempfile.TemporaryDirectory() as directory:
            path = os.path.join(directory, "host.sock")
            done = subprocess.run(
                [sys.executable, "-m", "rutis", path, directory],
                capture_output=True,
                text=True,
                timeout=TIMEOUT,
                env={**os.environ, "PYTHONPATH": package},
            )
        self.assertEqual(done.returncode, 1)
        self.assertEqual(done.stderr, f"a local channel needs --id <endpoint>: {path}; a host older than rutis 0.19 started this runtime: upgrade the host\n")


if __name__ == "__main__":
    unittest.main()
