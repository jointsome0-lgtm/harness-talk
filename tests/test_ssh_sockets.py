"""A reconnect may remove stale endpoints, never live sockets or other files."""
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "integrations/ssh_sockets.py"


class SocketPreparation(unittest.TestCase):
    def test_stale_pair_recovers_but_live_and_replaced_paths_are_preserved(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            mcp, watch = root / "mcp.sock", root / "watch.sock"

            def prepare():
                return subprocess.run([sys.executable, "-B", str(SCRIPT), tmp],
                                      capture_output=True, text=True, timeout=5)

            with socket.socket(socket.AF_UNIX) as stale:
                stale.bind(str(mcp))
            with socket.socket(socket.AF_UNIX) as live:
                live.bind(str(watch))
                live.listen()
                self.assertNotEqual(0, prepare().returncode)
                self.assertTrue(mcp.is_socket())
                self.assertTrue(watch.is_socket())
            self.assertEqual(0, prepare().returncode)
            self.assertFalse(mcp.exists())
            self.assertFalse(watch.exists())

            mcp.write_text("preserve this file")
            watch.symlink_to(mcp)
            self.assertNotEqual(0, prepare().returncode)
            self.assertEqual("preserve this file", mcp.read_text())
            self.assertTrue(watch.is_symlink())
