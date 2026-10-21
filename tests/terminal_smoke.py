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
import re
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

    def resize(self, width, height):
        self.screen.resize(lines=height, columns=width)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))

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
import json, os, re, sys
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
        row = dict(ID="id-" + name, Names=name, Image=name + ":test", State="running" if running else "exited", Status="Up (healthy)" if running else "Exited (0)", Ports="127.0.0.1:48992->80/tcp" if running else "", Project="fixture-project", Service=name, ConfigFiles=str(root / "my stack/compose.yaml") + "," + str(root / "my stack/override.yaml"), WorkingDir=str(root / "my stack"))
        labels = {"com.docker.compose.project": row["Project"], "com.docker.compose.service": row["Service"], "com.docker.compose.project.config_files": row["ConfigFiles"], "com.docker.compose.project.working_dir": row["WorkingDir"]}
        def json_field(match):
            expression = match.group(1).strip()
            label = re.fullmatch(r'\(\.Label "([^"]+)"\)', expression)
            value = labels.get(label.group(1), "") if label else row[expression.removeprefix(".")]
            return json.dumps(value)
        # Render the template efc actually passed, so broken JSON punctuation is
        # caught rather than hidden behind a hand-written JSON fixture response.
        output = re.sub(r'\{\{json (.*?)\}\}', json_field, args[-1])
        if os.environ.get("EFC_FIXTURE_BAD_PS"):
            output = output[:-1]
        print(output)
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
    assert args[1:] == ["--follow", "--timestamps", "--tail", "all", "--", "id-api"] or args[-1] == "id-redis"
    if os.environ.get("EFC_LARGE_LOGS"):
        sys.stdout.write("".join(f"2026-10-21T12:00:00Z HISTORY-{i:06} complete retained history " + "x" * 40 + "\n" for i in range(70000)))
    print("fixture standard output", flush=True)
    print("fixture standard error", file=sys.stderr, flush=True)
    if os.environ.get("EFC_LARGE_LOGS"):
        import time
        for i in range(1000):
            print(f"LIVE-{i:04} new output still being captured", flush=True)
            time.sleep(0.1)
elif args[0] == "inspect":
    if "--format" in args:
        assert args[args.index("--format") + 1] == "{{json .Config}}"
        print(json.dumps(dict(Tty=not bool(os.environ.get("EFC_ATTACH_CANONICAL")), StdinOnce=bool(os.environ.get("EFC_ATTACH_CANONICAL")))))
    else:
        print(json.dumps([dict(Name=args[-1], State=dict(Status="running"))]))
elif args[0] == "attach":
    import signal, tty
    expected = ["--sig-proxy=false", "--detach-keys=ctrl-p,ctrl-q"]
    if os.environ.get("EFC_ATTACH_CANONICAL"):
        expected.append("--no-stdin")
    assert args[1:] == expected + ["--", "id-api"]
    assert sys.stdin.isatty() and sys.stdout.isatty()
    if os.environ.get("EFC_ATTACH_FAIL"):
        print("fixture attach refused", file=sys.stderr, flush=True)
        sys.exit(42)
    if not os.environ.get("EFC_ATTACH_CANONICAL"):
        tty.setraw(sys.stdin.fileno())
    def resized(*_):
        size = os.get_terminal_size()
        os.write(1, f"\r\nATTACH RESIZE {size.columns}x{size.lines}\r\n".encode())
    signal.signal(signal.SIGWINCH, resized)
    os.write(1, b"\r\nATTACH READY\r\n")
    if os.environ.get("EFC_ATTACH_NOT_READING"):
        import time
        time.sleep(60)
    received = bytearray()
    while True:
        data = os.read(0, 4096)
        if not data:
            break
        with (root / "attach-input").open("ab") as out:
            out.write(data)
        os.write(1, b"\r\nATTACH INPUT " + data.hex().encode() + b"\r\n")
        received.extend(data)
        if b"exit\r" in received:
            break
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
        terminal.send("\x1b")
        terminal.until("Detached · redis")
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


def malformed_output_flow(root):
    env = dict(PATH=str(root) + os.pathsep + os.environ["PATH"], EFC_FIXTURE=str(root), EFC_FIXTURE_BAD_PS="1")
    terminal = Terminal([], env)
    try:
        terminal.until("Container data error")
        terminal.until("fake-context")
        assert "Docker is offline" not in terminal.text()
        assert "Engine is offline" not in terminal.text()
        assert "Docker replied" in terminal.text()
        terminal.quit()
    finally:
        terminal.close()
    print("PASS malformed container JSON is reported as a data error, with the engine still connected")


def attach_flow(root):
    env = dict(PATH=str(root) + os.pathsep + os.environ["PATH"], EFC_FIXTURE=str(root))
    terminal = Terminal([], env)
    try:
        terminal.until("fake-context")
        terminal.send("/api\r")
        terminal.pump()
        terminal.send("a")
        terminal.until("ATTACH READY")
        terminal.send("héllo\x1b[A\r")
        terminal.until("ATTACH INPUT")
        terminal.resize(124, 38)
        terminal.until("ATTACH RESIZE")
        assert "124x38" not in terminal.text(), "Child received whole-screen dimensions instead of its pane"
        terminal.send("\x11")
        terminal.until("Detached · api")
        terminal.until("fake-context")
        assert (root / "attach-input").read_bytes() == "héllo\x1b[A\r".encode()
        assert json.loads((root / "state.json").read_text())["api"], "Detach stopped the container"
        assert " / api " in terminal.text(), "Attach lost the selected filter"
        # Click the actual keycap after terminal restoration and resizing.
        for y, row in enumerate(terminal.screen.display):
            if "a Attach" in row:
                x = row.index("a Attach") + 4
                terminal.send(f"\x1b[<0;{x + 1};{y + 1}M\x1b[<0;{x + 1};{y + 1}m")
                break
        else:
            raise AssertionError("Attach button is missing")
        terminal.until("ATTACH READY")
        terminal.send("\x10")
        terminal.pump()
        terminal.send("\x11")
        terminal.until("Detached · api")
        terminal.send("a")
        terminal.until("ATTACH READY")
        terminal.send("exit\r")
        terminal.until("Attach session ended")
        terminal.until("Attached workspace / api")
        terminal.send("\x11")
        terminal.until("Detached · api")
        terminal.quit()
    finally:
        terminal.close()
    for extra, expected in [
        ({"EFC_ATTACH_CANONICAL": "1"}, "Detached · api"),
        ({"EFC_ATTACH_FAIL": "1"}, "Attach exited with status 42"),
    ]:
        terminal = Terminal([], dict(env, **extra))
        try:
            terminal.until("fake-context")
            terminal.send("a")
            if "EFC_ATTACH_FAIL" in extra:
                terminal.until(expected)
                terminal.until("fixture attach refused")
                terminal.send("\x11")
                terminal.until("Detached · api")
            else:
                terminal.until("ATTACH READY")
                terminal.until("Output-only session")
                terminal.send("ignored input\r")
                terminal.pump()
                terminal.send("\x03")
                terminal.until(expected)
            terminal.quit()
        finally:
            terminal.close()
    captured = (root / "attach-input").read_bytes()
    assert not any(key in captured for key in (b"\x03", b"\x10", b"\x11")), captured
    assert b"ignored input" not in captured, "Output-only attach forwarded stdin"
    print("PASS Attach button/shortcut, raw Unicode/arrows, resize, Ctrl+Q/Ctrl+C/Ctrl+P,Q detach, natural exit, failure and terminal restoration")


def embedded_history_flow(root):
    env = dict(PATH=str(root) + os.pathsep + os.environ["PATH"], EFC_FIXTURE=str(root), EFC_LARGE_LOGS="1")
    terminal = Terminal([], env)
    try:
        terminal.until("fake-context")
        terminal.send("a")
        terminal.until("Attached workspace / api")
        terminal.until("ATTACH READY")
        terminal.until("LIVE-")
        assert b"\x1b[?1049l" not in terminal.output, "Attach left the TUI's alternate screen"
        # Ctrl+W gives logs focus; Home reaches history beyond both old limits.
        terminal.send("\x17\x1b[H")
        terminal.until("HISTORY-000000")
        def total():
            return int(re.search(r"(\d+) lines", terminal.text()).group(1))
        before = total()
        assert before > 70000, before
        terminal.pump(0.8)
        assert "HISTORY-000000" in terminal.text(), "Live output moved the paused viewport"
        assert total() > before, "Paused history stopped capturing new output"
        terminal.send("\x1b[6~")
        terminal.until("HISTORY-0000")
        assert "HISTORY-000000" not in terminal.text(), "PageDown did not scroll"
        terminal.send("/HISTORY-010123\r")
        terminal.until("Match on line 10124")
        terminal.until("HISTORY-010123")
        terminal.send("\x1b[F")
        terminal.until("LIVE-")
        terminal.until("following")
        # Click inside the terminal: subsequent ordinary keys go to the process.
        row = next(y for y, line in enumerate(terminal.screen.display) if "Terminal ·" in line)
        terminal.send(f"\x1b[<0;5;{row + 3}M\x1b[<0;5;{row + 3}m")
        terminal.send("\x1b[200~pasted é\t\x1b[201~")
        terminal.until("ATTACH INPUT")
        terminal.pump(0.2)
        assert b"pasted \xc3\xa9\t" in (root / "attach-input").read_bytes()
        # While focused in the terminal, PageUp safely switches to log history.
        terminal.send("\x1b[5~")
        terminal.until("paused")
        terminal.send("\x11")
        terminal.until("Detached · api")
        terminal.quit()
    finally:
        terminal.close()
    print("PASS embedded panes, 70,000+ historical lines, >2 MiB, Home/End/pages, pause with live capture, full-history search, mouse focus, Unicode paste and no terminal handoff")


def blocked_input_flow(root):
    env = dict(PATH=str(root) + os.pathsep + os.environ["PATH"], EFC_FIXTURE=str(root), EFC_ATTACH_NOT_READING="1")
    terminal = Terminal([], env)
    try:
        terminal.until("fake-context")
        terminal.send("a")
        terminal.until("ATTACH READY")
        terminal.send("\x1b[200~" + "x" * 32768 + "\x1b[201~")
        terminal.pump(0.2)
        terminal.send("\x11")
        terminal.until("Detached · api", timeout=3)
        terminal.quit()
    finally:
        terminal.close()
    print("PASS unresponsive container stdin cannot block UI or detach")


if __name__ == "__main__":
    assert BINARY.exists(), "Run cargo build first"
    demo_flow()
    with tempfile.TemporaryDirectory(prefix="efc-qa-") as directory:
        live_fixture_flow(Path(directory))
        malformed_output_flow(Path(directory))
        attach_flow(Path(directory))
        embedded_history_flow(Path(directory))
        blocked_input_flow(Path(directory))
