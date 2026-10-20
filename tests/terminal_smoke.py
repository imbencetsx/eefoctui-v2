#!/usr/bin/env python3
"""Exercise the actual efc binary in a PTY, without touching a real Docker engine.

Run: cargo build && uv run --with pyte==0.8.2 python tests/terminal_smoke.py
macOS/Linux. pyte reconstructs the screen from Ratatui's incremental updates.
A temporary Docker CLI fixture checks
command dispatch, while efc's host resource and socket collectors remain real.
"""
import codecs
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import tempfile
import termios
import time
import pyte

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target/debug/efc"


class Terminal:
    def __init__(self, args, env=None, width=140, height=42):
        self.output = b""
        self.screen = pyte.Screen(width, height)
        self.stream = pyte.Stream(self.screen)
        self.decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            child_env = dict(os.environ, TERM="xterm-256color")
            child_env.update(env or {})
            os.execve(str(BINARY), [str(BINARY), *args], child_env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        self.status = None

    def send(self, keys):
        os.write(self.fd, keys.encode())

    def pump(self, duration=0.15):
        end = time.monotonic() + duration
        while time.monotonic() < end:
            if select.select([self.fd], [], [], min(0.05, max(0, end - time.monotonic())))[0]:
                try:
                    chunk = os.read(self.fd, 65536)
                    if not chunk:
                        break
                    self.output += chunk
                    self.stream.feed(self.decoder.decode(chunk))
                except OSError as error:
                    if error.errno == errno.EIO:
                        break
                    raise

    def text(self):
        return "\n".join(self.screen.display)

    def until(self, expected, timeout=12, after=0):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            self.pump()
            if expected in self.text()[after:]:
                return
        raise AssertionError(f"Missing {expected!r} in terminal output:\n{self.text()[-2500:]}")

    def quit(self):
        self.send("\x11")  # Ctrl+Q, also works inside every popup.
        end = time.monotonic() + 5
        while time.monotonic() < end:
            self.pump()
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.status = status
                assert os.waitstatus_to_exitcode(status) == 0, self.text()[-2000:]
                assert b"\x1b[?1049l" in self.output, "Alternate screen was not restored"
                os.close(self.fd)
                return
        raise AssertionError("Ctrl+Q did not exit promptly")

    def close(self):
        if self.status is None:
            try:
                os.kill(self.pid, signal.SIGKILL)
                os.waitpid(self.pid, 0)
            except ProcessLookupError:
                pass
            os.close(self.fd)


FIXTURE = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
root = Path(os.environ["EFC_FIXTURE"])
with (root / "calls.jsonl").open("a") as out:
    out.write(json.dumps(args) + "\n")
state_path = root / "state.json"
state = json.loads(state_path.read_text())
if args[0] == "ps":
    assert "--all" in args and "--no-trunc" in args
    assert "com.docker.compose.project.config_files" in args[-1]
    for name, running in state.items():
        print(json.dumps(dict(ID="id-" + name, Names=name, Image=name + ":test", State="running" if running else "exited", Status="Up (healthy)" if running else "Exited (0)", Ports="127.0.0.1:48992->80/tcp" if running else "", Project="fixture-project", Service=name, ConfigFiles=str(root / "my stack/compose.yaml") + "," + str(root / "my stack/override.yaml"), WorkingDir=str(root / "my stack"))))
elif args[0] == "stats":
    for name, running in state.items():
        if running:
            print(json.dumps(dict(Name=name, CPUPerc="12.3%", MemUsage="24MiB / 2GiB", MemPerc="1.2%", NetIO="1kB / 2kB", BlockIO="0B / 0B", PIDs="4")))
elif args[0] == "version":
    print("99.0-fixture")
elif args[0] == "context":
    print("fake-context")
elif args[0] in ("start", "stop", "restart"):
    name = args[-1].removeprefix("id-")
    state[name] = args[0] != "stop"
    state_path.write_text(json.dumps(state))
    print(name)
elif args[0] == "logs":
    print("fixture standard output")
    print("fixture standard error", file=sys.stderr)
elif args[0] == "inspect":
    print(json.dumps([dict(Name=args[-1], State=dict(Status="running"))]))
elif args[0] == "compose":
    if "config" in args:
        print(json.dumps(dict(name="fixture-project", services=dict(api={}, db={})))); sys.exit(0)
    assert args[args.index("--project-name") + 1] == "fixture-project"
    assert str(root / "my stack/compose.yaml") in args
    assert str(root / "my stack/override.yaml") in args
    for name in state:
        state[name] = "stop" not in args
    state_path.write_text(json.dumps(state))
else:
    sys.exit("Unexpected fixture command: " + repr(args))
'''


def calls(root):
    path = root / "calls.jsonl"
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def wait_call(terminal, root, predicate, timeout=12):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        terminal.pump()
        if any(predicate(call) for call in calls(root)):
            return
    raise AssertionError(f"Expected Docker command was not called: {calls(root)}")


def demo_flow():
    terminal = Terminal(["--demo"])
    try:
        terminal.until("gateway")
        terminal.send("\x03")
        terminal.until("Use Ctrl+Q to quit efc.")
        assert os.waitpid(terminal.pid, os.WNOHANG)[0] == 0
        terminal.send("\r")
        terminal.pump(0.2)
        # SGR mouse press/release over the Compose tab (1-based coordinates).
        terminal.send("\x1b[<0;25;5M\x1b[<0;25;5m")
        terminal.until("Compose projects")
        terminal.send("3")
        terminal.until("Listening ports & bindings")
        terminal.send("4")
        terminal.until("System information")
        terminal.send("1/redis\rl")
        terminal.until("simulated logs")
        terminal.quit()
    finally:
        terminal.close()
    print("PASS demo mouse/keyboard navigation, filtering, logs, Ctrl+C reminder, Ctrl+Q and restoration")


def live_fixture_flow(root):
    (root / "docker").write_text(FIXTURE)
    (root / "docker").chmod(0o755)
    (root / "state.json").write_text(json.dumps(dict(api=True, db=False)))
    env = dict(PATH=str(root) + os.pathsep + os.environ["PATH"], EFC_FIXTURE=str(root))
    # A real local listener must appear in efc's host scan.
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        port = str(listener.getsockname()[1])
        terminal = Terminal([], env)
        try:
            terminal.until("fake-context")
            terminal.until("12.3%")
            terminal.send("x")
            terminal.until("Stop api?")
            terminal.send("\r")
            terminal.pump(0.3)
            assert not any(c[0] == "stop" for c in calls(root)), "Cancel dispatched a stop"
            terminal.send("xy")
            wait_call(terminal, root, lambda c: c == ["stop", "--", "id-api"])
            terminal.until("Stop completed")
            terminal.pump(0.3)
            terminal.send("s")
            wait_call(terminal, root, lambda c: c == ["start", "--", "id-api"])
            terminal.until("Start completed")
            terminal.send("l")
            terminal.until("fixture standard output")
            terminal.until("fixture standard error")
            terminal.send("\x1b")
            terminal.pump(0.2)
            terminal.send("i")
            terminal.until("Inspect · api")
            terminal.send("\x1b")
            terminal.pump(0.2)
            terminal.send("2xy")
            wait_call(terminal, root, lambda c: c[0] == "compose" and c[-1] == "stop")
            terminal.until("Stop completed")
            terminal.pump(0.3)
            terminal.send("uy")
            wait_call(terminal, root, lambda c: c[0] == "compose" and c[-3:] == ["up", "--detach", "--no-recreate"])
            terminal.until("Bring up completed")
            terminal.send("3/" + port + "\r")
            terminal.until(port)
            terminal.until(str(os.getpid()))
            terminal.quit()
        finally:
            terminal.close()
    print("PASS Docker stats, safe cancel, start/stop, both log streams, inspect, multi-file Compose stop/up, real host port")


if __name__ == "__main__":
    assert BINARY.exists(), "Run cargo build first"
    demo_flow()
    with tempfile.TemporaryDirectory(prefix="efc-qa-") as directory:
        live_fixture_flow(Path(directory))
