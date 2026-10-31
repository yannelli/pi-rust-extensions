#!/usr/bin/env python3
"""Quit a real pi TUI while a published Lens extension holds a stalled LSP, and check that no child survives.

Installs `marketplace:lens@<tag>` with the given pi into isolated dirs under --out, answers model calls from a
loopback mock, starts the stalled LSP through a `write app.py` turn, then quits with Ctrl+D and with SIGTERM.
Passes when both quits exit 0 within 12 s and the Lens process, the LSP, and its child are gone 3 s later.
"""

import argparse
import fcntl
import hashlib
import json
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path

from mock import MockOpenAI

HERE = Path(__file__).resolve().parent
EXIT_LIMIT = 12.0
CHILD_LIMIT = 3.0


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def command_of(pid):
    done = subprocess.run(["ps", "-ww", "-p", str(pid), "-o", "command="], capture_output=True, text=True, check=False)
    return done.stdout.strip() if done.returncode == 0 else None


def owned(pid, expected):
    """Whether `pid` still runs exactly the command line recorded for it, so cleanup kills only this probe's."""
    return alive(pid) and command_of(pid) == expected


def find_extension(lens):
    """The pid of the one process running the installed Lens binary of this case with its manifest args."""
    expected = " ".join([lens["binary"], *lens["args"]])
    done = subprocess.run(["ps", "-ww", "-A", "-o", "pid=,command="], capture_output=True, text=True, check=False)
    pids = [int(pid) for pid, _, command in (line.strip().partition(" ") for line in done.stdout.splitlines())
            if command.strip() == expected]
    if len(pids) != 1:
        raise RuntimeError(f"expected one running {expected!r}, found {pids}")
    return pids[0], expected


def base_env(case):
    env = {"PATH": "/usr/bin:/bin", "HOME": str(case / "home"), "TMPDIR": str(case / "tmp"), "LANG": "en_US.UTF-8",
           "PI_CODING_AGENT_DIR": str(case / "agent"), "PI_SKIP_VERSION_CHECK": "1"}
    for name in ("home", "tmp", "agent", "work"):
        (case / name).mkdir(parents=True, exist_ok=True)
    return env


def run(case, name, args, env, timeout):
    """Run one command and keep its raw output and status, also when it times out or cannot start."""
    status = {"args": [str(arg) for arg in args], "exit": None, "timedOut": False, "error": None}
    out = err = b""
    try:
        done = subprocess.run(args, cwd=case / "work", env=env, capture_output=True, timeout=timeout, check=False)
        status["exit"], out, err = done.returncode, done.stdout, done.stderr
    except subprocess.TimeoutExpired as expired:
        status["timedOut"], out, err = True, expired.stdout or b"", expired.stderr or b""
    except OSError as error:
        status["error"] = f"{type(error).__name__}: {error}"
    (case / f"{name}.stdout").write_bytes(out)
    (case / f"{name}.stderr").write_bytes(err)
    (case / f"{name}.status.json").write_text(json.dumps(status, indent=2) + "\n")
    return status["exit"], (out + err).decode("utf-8", "replace")


def install(case, pi, tag):
    """Install the published Lens release; pi checks the index signature and the archive digest."""
    code, text = run(case, "install", [pi, "install", f"marketplace:lens@{tag}"], base_env(case), 600)
    if code != 0 or f"Installed marketplace:lens@{tag}" not in text:
        raise RuntimeError(f"pi install failed ({code}): {text[-400:]}")
    manifest = json.loads((case / "agent/marketplace/packages/lens/package.json").read_text())
    state = manifest["piMarketplace"]
    binary = Path(manifest["pi"]["native"][0]["command"])
    marker = Path(f"{binary}.sha256").read_text().strip()
    if state["tag"] != tag or state["name"] != "lens" or marker != state["sha256"] or not binary.is_file():
        raise RuntimeError(f"unexpected install state {state}")
    return {"binary": str(binary), "binarySha256": sha256(binary), "archiveSha256": state["sha256"],
            "version": state["version"], "target": state["target"], "args": manifest["pi"]["native"][0]["args"]}


def configure(case, mock, version, pidfile):
    agent = case / "agent"
    models = {"providers": {"mock": {"baseUrl": mock.url, "api": "openai-completions", "apiKey": "mock-key",
                                     "models": [{"id": "mock-model", "contextWindow": 100000, "maxTokens": 4000}]}}}
    (agent / "models.json").write_text(json.dumps(models))
    settings = json.loads((agent / "settings.json").read_text())
    settings.update({"defaultProvider": "mock", "defaultModel": "mock-model", "defaultProjectTrust": "always",
                     "lastChangelogVersion": version})
    (agent / "settings.json").write_text(json.dumps(settings))
    server = {"command": sys.executable, "args": [str(HERE / "stalled_lsp.py"), str(pidfile)], "languages": ["python"]}
    config = {"autoDetect": False, "timeoutMs": 120000, "pipelineTimeoutMs": 120000, "lsp": {"servers": [server]}}
    (case / "lens.json").write_text(json.dumps(config))
    mock.turns[:] = [{"tools": [{"name": "write", "args": {"path": "app.py", "content": "x = 1\n"}}]},
                     {"text": "done"}]


class Screen:
    def __init__(self, master):
        self.master, self.data = master, bytearray()

    def drain(self, wait):
        if select.select([self.master], [], [], wait)[0]:
            try:
                self.data.extend(os.read(self.master, 65536))
            except OSError:
                pass

    def wait_for(self, done, limit, process):
        deadline = time.monotonic() + limit
        while not done() and time.monotonic() < deadline and process.poll() is None:
            self.drain(0.1)
        return done()


def quit_case(case, pi, mode, mock, version, lens):
    pidfile = case / "pids.json"
    configure(case, mock, version, pidfile)
    env = base_env(case) | {"PI_LENS_CONFIG_PATH": str(case / "lens.json"), "PI_OFFLINE": "1",
                            "TERM": "xterm-256color"}
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
    process = subprocess.Popen([pi, "--offline"], cwd=case / "work", env=env, stdin=slave, stdout=slave,
                               stderr=slave, start_new_session=True)
    os.close(slave)
    screen, commands, result = Screen(master), {}, {"mode": mode}
    try:
        ready = lambda: all(marker in screen.data for marker in (b"mock-model", b"full startup help", b"[Extensions]"))
        if not screen.wait_for(ready, 30, process):
            raise RuntimeError("the TUI did not finish startup with the mock model and extensions")
        os.write(master, b"Write app.py")
        screen.drain(0.2)
        os.write(master, b"\r")
        if not screen.wait_for(pidfile.exists, 30, process):
            raise RuntimeError("Lens never started the stalled LSP")
        pids = json.loads(pidfile.read_text())
        pids["extension"], extension_command = find_extension(lens)
        expected = {"lsp": lambda command: str(HERE / "stalled_lsp.py") in command and str(pidfile) in command,
                    "child": lambda command: command == "/bin/sleep 600",
                    "extension": lambda command: command == extension_command}
        recorded = {key: command_of(pids[key]) for key in expected}
        commands = {pids[key]: recorded[key] for key, matches in expected.items() if recorded[key] and matches(recorded[key])}
        if len(commands) != len(expected):
            raise RuntimeError(f"the recorded processes do not run the expected commands: {recorded}")
        known = list(commands)
        started = time.monotonic()
        if mode == "ctrld":
            os.write(master, b"\x04")
        else:
            process.send_signal(signal.SIGTERM)
        screen.wait_for(lambda: process.poll() is not None, EXIT_LIMIT, process)
        elapsed = time.monotonic() - started
        deadline = time.monotonic() + CHILD_LIMIT
        while any(alive(pid) for pid in known) and time.monotonic() < deadline:
            screen.drain(0.1)
        result.update({"exit": process.poll(), "exitSeconds": round(elapsed, 3), "pids": pids,
                       "commands": {str(pid): command for pid, command in commands.items()},
                       "survivors": [pid for pid in known if alive(pid)]})
    except RuntimeError as error:
        result["error"] = str(error)
    finally:
        (case / "capture.txt").write_bytes(bytes(screen.data))
        for pid, command in commands.items():
            if owned(pid, command):
                os.kill(pid, signal.SIGKILL)
        if process.poll() is None:
            try:
                leader = os.getpgid(process.pid) == process.pid
            except ProcessLookupError:
                leader = False
            if leader and process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
        process.wait(timeout=10)
        os.close(master)
    return result


def passed(result):
    return (result.get("exit") == 0 and result.get("exitSeconds", EXIT_LIMIT + 1) <= EXIT_LIMIT
            and result.get("survivors") == [] and "error" not in result)


def main():
    parser = argparse.ArgumentParser(description="Lens stalled-LSP quit probe with a published pi and Lens.")
    parser.add_argument("--pi", required=True)
    parser.add_argument("--pi-sha256", required=True)
    parser.add_argument("--marketplace-tag", required=True)
    parser.add_argument("--out", required=True, help="new directory that the probe owns")
    args = parser.parse_args()
    if not re.fullmatch(r"v\d+\.\d+\.\d+", args.marketplace_tag):
        sys.exit(f"invalid marketplace tag {args.marketplace_tag}")
    pi = str(Path(args.pi).resolve())
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=False)
    report = {"pi": pi, "piSha256": sha256(pi), "marketplaceTag": args.marketplace_tag, "results": []}
    try:
        if report["piSha256"] != args.pi_sha256:
            raise RuntimeError(f"pi sha256 {report['piSha256']} is not {args.pi_sha256}")
        code, version = run(out, "pi-version", [pi, "--version"], base_env(out) | {"PI_OFFLINE": "1"}, 60)
        report["piVersion"] = version.strip()
        if code != 0:
            raise RuntimeError(f"pi --version exited {code}")
        mock = MockOpenAI([])
        try:
            for mode in ("ctrld", "sigterm"):
                case = out / mode
                report.setdefault("lens", {})[mode] = install(case, pi, args.marketplace_tag)
                lens = report["lens"][mode]
                report["results"].append(quit_case(case, pi, mode, mock, report["piVersion"], lens))
        finally:
            report["modelRequests"] = len(mock.requests)
            mock.close()
    except (RuntimeError, OSError, KeyError, ValueError, subprocess.TimeoutExpired) as error:
        report["error"] = f"{type(error).__name__}: {error}"
    hashes = {lens["binarySha256"] for lens in report.get("lens", {}).values()}
    report["passed"] = ("error" not in report and len(report["results"]) == 2 and len(hashes) == 1
                        and all(map(passed, report["results"])))
    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
