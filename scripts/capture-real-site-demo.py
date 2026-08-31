#!/usr/bin/env python3
"""Record website demos from real shells and native TUIs.

The recorder runs asciinema inside a private tmux session and drives that
session with tmux send-keys.  The published cast is therefore the terminal
output produced by the real installer, Open Agent View binary, and provider
CLI—not HTML made to resemble those tools.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import re
import shlex
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from compact_real_recordings import (
    CompressionInterval,
    compact_recording,
    retime_recording_intervals,
)


COLS = 132
ROWS = 34
PROJECT_ROOT = Path(__file__).resolve().parents[1]
EXPECTED_VERSION_MATCH = re.search(
    r'^version\s*=\s*"([^"]+)"',
    (PROJECT_ROOT / "Cargo.toml").read_text(encoding="utf-8"),
    re.MULTILINE,
)
if EXPECTED_VERSION_MATCH is None:
    raise RuntimeError("could not read the package version from Cargo.toml")
EXPECTED_VERSION = EXPECTED_VERSION_MATCH.group(1)
APP_HEADER = f"Open Agent View v{EXPECTED_VERSION}"
APP_HEADER_PATTERN = re.escape(APP_HEADER)
INSTALL_COMMAND = "curl -fsSL https://open-agent-view.github.io/install.sh | bash"
SECRET_PATTERN = re.compile(
    r"api[_-]?key|oauth[_-]?token|authorization:\s*bearer|"
    r"(?<![A-Za-z0-9])ghp_[A-Za-z0-9]{8,}|"
    r"(?<![A-Za-z0-9])sk-[A-Za-z0-9_-]{8,}",
    re.I,
)
EMAIL_PATTERN = re.compile(
    r"\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b",
    re.I,
)
ANSI_EMAIL_PATTERN = re.compile(
    r"(\x1b\[[0-9;?]*m)([A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,})",
    re.I,
)
ONE_TIME_LOGIN_URL_PATTERN = re.compile(
    r"https://[^\s\x1b\"']*(?:oauth|authorize|login|signin|sign-in)[^\s\x1b\"']*",
    re.I,
)
DEVICE_CODE_PATTERN = re.compile(r"\b[A-Z0-9]{4}-[A-Z0-9]{4}\b")
RECORDER_SECRET_VALUES: set[str] = set()


def run(
    args: list[str],
    *,
    capture: bool = False,
    check: bool = True,
    timeout: float = 120,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        check=check,
        capture_output=capture,
        text=True,
        timeout=timeout,
    )


def require_program(name: str) -> str:
    value = shutil.which(name)
    if value is None:
        raise RuntimeError(f"required real-demo program is not installed: {name}")
    return value


def cast_records(path: Path) -> tuple[dict[str, Any], list[list[Any]]]:
    if not path.is_file():
        return {}, []
    lines = path.read_text(encoding="utf-8", errors="strict").splitlines()
    if not lines:
        return {}, []
    return json.loads(lines[0]), [json.loads(line) for line in lines[1:] if line]


def cast_time(path: Path) -> float:
    _, events = cast_records(path)
    return float(events[-1][0]) if events else 0.0


def write_trimmed_cast(
    source: Path,
    target: Path,
    start: float,
    end: float,
    replacements: dict[str, str] | None = None,
) -> None:
    header, events = cast_records(source)
    kept = [event for event in events if start <= float(event[0]) <= end]
    if not header or not kept:
        raise RuntimeError("real terminal capture did not produce an asciicast")
    for event in kept:
        event[0] = round(float(event[0]) - start, 6)
        if event[1] != "o" or not replacements:
            continue
        for source_text, target_text in replacements.items():
            event[2] = str(event[2]).replace(source_text, target_text)
        event[2] = ANSI_EMAIL_PATTERN.sub(
            lambda match: f"{match.group(1)}signed-in account",
            str(event[2]),
        )
        event[2] = EMAIL_PATTERN.sub("signed-in account", str(event[2]))
        event[2] = ONE_TIME_LOGIN_URL_PATTERN.sub(
            "[one-time sign-in link redacted]",
            str(event[2]),
        )
        event[2] = DEVICE_CODE_PATTERN.sub("[device code redacted]", str(event[2]))
    kept.append([round(end - start + 1.35, 6), "o", "\x1b[0m"])
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(
        "\n".join(json.dumps(record, ensure_ascii=False) for record in (header, *kept))
        + "\n",
        encoding="utf-8",
    )
    target.chmod(0o644)


def prime_first_terminal_frame(target: Path) -> float:
    """Make the inherited TUI state visible at time zero.

    tmux emits a resize followed by a burst of full-screen repaint events. If
    that burst retains its recorder startup delay, an embedded player shows a
    blank shell until playback begins. Collapse only that first repaint burst
    to time zero and preserve every later delay/action at its real offset.
    """

    header, events = cast_records(target)
    first_output = next(
        (float(event[0]) for event in events if event[1] == "o" and event[2]),
        None,
    )
    if first_output is None or first_output <= 0.001:
        return 0.0

    repaint_cutoff = first_output + 0.05
    for event in events:
        event_time = float(event[0])
        event[0] = (
            0.0
            if event_time <= repaint_cutoff
            else round(event_time - repaint_cutoff, 6)
        )
    target.write_text(
        "\n".join(json.dumps(record, ensure_ascii=False) for record in (header, *events))
        + "\n",
        encoding="utf-8",
    )
    target.chmod(0o644)
    return repaint_cutoff


def visible_cast(path: Path) -> str:
    _, events = cast_records(path)
    text = "".join(str(event[2]) for event in events if event[1] == "o")
    text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
    text = re.sub(r"\x1b\][^\x07]*(?:\x07|\x1b\\)", "", text)
    return re.sub(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]", "", text)


def terminate_owned_processes(root: Path) -> None:
    """Terminate only processes whose argv or cwd proves this capture owns them."""

    marker = str(root).encode()
    owned: list[int] = []
    proc = Path("/proc")
    if not proc.is_dir():
        return
    for entry in proc.iterdir():
        if not entry.name.isdigit() or int(entry.name) == os.getpid():
            continue
        try:
            command = (entry / "cmdline").read_bytes()
            cwd = os.readlink(entry / "cwd").encode()
        except (FileNotFoundError, PermissionError, ProcessLookupError, OSError):
            continue
        if marker in command or marker in cwd:
            owned.append(int(entry.name))
    for sig in (signal.SIGTERM, signal.SIGKILL):
        for pid in owned:
            with contextlib.suppress(ProcessLookupError):
                os.kill(pid, sig)
        time.sleep(0.25)


class RealTerminal:
    def __init__(self, name: str, root: Path, environment: dict[str, str]) -> None:
        self.session = f"oav-site-{name}-{os.getpid()}"
        self.root = root
        self.raw_cast = root / f"{name}.raw.cast"
        self.actions: list[dict[str, Any]] = []
        self.timing_marks: list[dict[str, Any]] = []
        self.closed = False
        run(["tmux", "kill-session", "-t", self.session], check=False)
        run(
            [
                "tmux",
                "new-session",
                "-d",
                "-s",
                self.session,
                "-x",
                str(COLS),
                "-y",
                str(ROWS),
                "-c",
                str(root / "home" / "work" / "acme-dashboard"),
            ]
        )
        # Never place login/API material in the long-lived Asciinema process
        # argv. Keep the complete environment in this owned mode-0600 file and
        # expose only its disposable path to the recorded shell command.
        environment_file = root / "recording.env"
        environment_file.write_text(
            "\n".join(
                f"export {key}={shlex.quote(value)}"
                for key, value in sorted(environment.items())
            )
            + "\n",
            encoding="utf-8",
        )
        environment_file.chmod(0o600)
        inner_shell = (
            f". {shlex.quote(str(environment_file))}; "
            "exec bash --noprofile --norc"
        )
        command = (
            f"{shlex.quote(require_program('asciinema'))} rec "
            f"--overwrite --quiet "
            f"--command {shlex.quote(inner_shell)} "
            f"{shlex.quote(str(self.raw_cast))}"
        )
        run(["tmux", "send-keys", "-t", self.session, "-l", command])
        run(["tmux", "send-keys", "-t", self.session, "Enter"])
        self.wait_for_recorded("OAV-DEMO-READY", 20)
        self.clock_wall = time.monotonic()
        self.clock_cast = cast_time(self.raw_cast)

    def pane(self, history: int = 120) -> str:
        return run(
            [
                "tmux",
                "capture-pane",
                "-t",
                self.session,
                "-p",
                "-S",
                f"-{history}",
            ],
            capture=True,
        ).stdout

    def screen(self) -> str:
        return run(
            ["tmux", "capture-pane", "-t", self.session, "-p"], capture=True
        ).stdout

    def styled_screen(self) -> str:
        return run(
            ["tmux", "capture-pane", "-t", self.session, "-p", "-e"],
            capture=True,
        ).stdout

    def wait_selected_row(self, provider: str, timeout: float = 10) -> None:
        """Require OAV's selected-row background on the launched provider."""

        selected_background = "\x1b[48;2;58;60;61m"
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for line in self.styled_screen().splitlines():
                if selected_background in line and provider in line:
                    return
            time.sleep(0.2)
        tail = self.styled_screen()[-5000:].replace(str(Path.home()), "~")
        raise RuntimeError(
            f"new {provider} row appeared but was not selected after launch; "
            f"styled screen={tail!r}"
        )

    def wait_for(self, pattern: str, timeout: float = 90) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if re.search(pattern, self.pane(), re.I | re.S):
                return
            time.sleep(0.25)
        tail = self.pane()[-4000:].replace(str(Path.home()), "~")
        raise RuntimeError(f"real terminal never rendered /{pattern}/; pane={tail!r}")

    def wait_screen(self, pattern: str, timeout: float = 45) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if re.search(pattern, self.screen(), re.I | re.S):
                return
            time.sleep(0.25)
        tail = self.screen()[-4000:].replace(str(Path.home()), "~")
        raise RuntimeError(f"real terminal screen never rendered /{pattern}/; screen={tail!r}")

    def wait_screen_without(self, pattern: str, timeout: float = 45) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not re.search(pattern, self.screen(), re.I | re.S):
                return
            time.sleep(0.25)
        tail = self.screen()[-4000:].replace(str(Path.home()), "~")
        raise RuntimeError(
            f"real terminal screen kept rendering /{pattern}/; screen={tail!r}"
        )

    def wait_native_screen(self, pattern: str, timeout: float = 45) -> None:
        """Require a provider marker while the OAV dashboard is absent."""

        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            screen = self.screen()
            if re.search(pattern, screen, re.I | re.S) and not re.search(
                APP_HEADER_PATTERN, screen, re.I | re.S
            ):
                return
            time.sleep(0.25)
        tail = self.screen()[-4000:].replace(str(Path.home()), "~")
        raise RuntimeError(
            f"real native terminal never rendered /{pattern}/ away from OAV; "
            f"screen={tail!r}"
        )

    def wait_screen_settled(
        self,
        baseline: str,
        *,
        provider: str,
        timeout: float = 240,
        minimum_wait: float = 8,
        stable_for: float = 6,
    ) -> None:
        """Wait for a real native turn to change the screen and finish repainting."""

        started = time.monotonic()
        deadline = started + timeout
        changed = False
        stable_since = started
        previous = baseline
        trust_answered = False
        copilot_url_answered = False
        terminal_setup_closed = False
        while time.monotonic() < deadline:
            screen = self.screen()
            if (
                not copilot_url_answered
                and "Do you want to allow this access?" in screen
                and "Copilot is attempting to access the following URL" in screen
            ):
                self.key(
                    "Enter",
                    "Enter · allow this URL once",
                    provider,
                )
                copilot_url_answered = True
                time.sleep(0.8)
                previous = self.screen()
                stable_since = time.monotonic()
                continue
            if not trust_answered and (
                "Workspace Trust Required" in screen
                or "Confirm folder trust" in screen
                or "Do you trust the files in this folder" in screen
                or "Trust this folder?" in screen
            ):
                if "Confirm folder trust" in screen:
                    key = "Enter"
                elif "Trust this folder?" in screen:
                    key = "Enter"
                elif "Do you trust the files in this folder" in screen:
                    key = "a"
                else:
                    key = "a"
                self.key(key, "Confirm · trust demo workspace", provider)
                trust_answered = True
                time.sleep(0.8)
                previous = self.screen()
                stable_since = time.monotonic()
                continue
            if not terminal_setup_closed and "Set up terminal for multi-line input support" in screen:
                self.key("Escape", "Esc · keep terminal settings", provider)
                terminal_setup_closed = True
                time.sleep(0.8)
                previous = self.screen()
                stable_since = time.monotonic()
                continue
            if screen != baseline:
                changed = True
            if screen != previous:
                previous = screen
                stable_since = time.monotonic()
            now = time.monotonic()
            if (
                changed
                and now - started >= minimum_wait
                and now - stable_since >= stable_for
            ):
                return
            time.sleep(0.35)
        tail = self.screen()[-4000:].replace(str(Path.home()), "~")
        raise RuntimeError(
            f"real {provider} turn did not settle after {timeout:.0f}s; screen={tail!r}"
        )

    def wait_screen_occurrences(self, text: str, count: int = 2, timeout: float = 120) -> None:
        deadline = time.monotonic() + timeout
        trust_answered = False
        while time.monotonic() < deadline:
            screen = self.screen()
            if not trust_answered and (
                "Workspace Trust Required" in screen
                or "Confirm folder trust" in screen
            ):
                trust_key = "Enter" if "Confirm folder trust" in screen else "a"
                self.key(
                    trust_key,
                    "Confirm · trust disposable demo workspace",
                    "native harness",
                )
                trust_answered = True
                time.sleep(0.5)
                continue
            if screen.count(text) >= count:
                return
            time.sleep(0.25)
        tail = self.screen()[-4000:].replace(str(Path.home()), "~")
        raise RuntimeError(
            f"real terminal did not render {count} copies of {text!r}; screen={tail!r}"
        )

    def wait_for_recorded(self, marker: str, timeout: float = 20) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.raw_cast.is_file() and marker in self.raw_cast.read_text(
                encoding="utf-8", errors="replace"
            ):
                return
            time.sleep(0.1)
        size = self.raw_cast.stat().st_size if self.raw_cast.is_file() else 0
        raise RuntimeError(
            f"asciinema did not start its real shell ({marker}); "
            f"cast_bytes={size}; pane={self.pane()[-2000:]!r}"
        )

    def remember(self, label: str, window: str) -> None:
        time.sleep(0.12)
        self.actions.append(
            {
                "at": round(self.timeline_time(), 3),
                "action": label,
                "window": window,
            }
        )

    def mark_timing(self, label: str) -> None:
        """Record a private timing boundary that is not shown as a keycap."""

        self.timing_marks.append(
            {
                "at": self.timeline_time(),
                "label": label,
            }
        )

    def wait_codex_response_started(self, baseline: str, timeout: float = 120) -> None:
        """Wait until Codex replaces Working with its actual answer text."""

        def response_lines(screen: str) -> set[str]:
            return {
                line.strip()
                for line in screen.splitlines()
                if re.match(r"^[•·]\s+(?!Working\b).+", line.strip(), re.I)
            }

        before = response_lines(baseline)
        deadline = time.monotonic() + timeout
        ready_since: float | None = None
        while time.monotonic() < deadline:
            screen = self.screen()
            current = response_lines(screen)
            if (
                current - before
                and not re.search(r"Working\s*\([^\n]*esc to interrupt", screen, re.I)
            ):
                ready_since = ready_since or time.monotonic()
                if time.monotonic() - ready_since >= 0.2:
                    return
            else:
                ready_since = None
            time.sleep(0.08)
        tail = self.screen()[-4000:].replace(str(Path.home()), "~")
        raise RuntimeError(
            f"real Codex turn never replaced Working with response output; screen={tail!r}"
        )

    def timeline_time(self) -> float:
        return self.clock_cast + (time.monotonic() - self.clock_wall)

    def repaint_start(self) -> float:
        before = cast_time(self.raw_cast)
        run(
            ["tmux", "resize-window", "-t", self.session, "-x", str(COLS - 1), "-y", str(ROWS)]
        )
        time.sleep(0.25)
        run(
            ["tmux", "resize-window", "-t", self.session, "-x", str(COLS), "-y", str(ROWS)]
        )
        time.sleep(0.5)
        return before + 0.000001

    def key(self, key: str, label: str, window: str) -> None:
        run(["tmux", "send-keys", "-t", self.session, key])
        self.remember(label, window)

    def literal_key(self, sequence: str, label: str, window: str) -> None:
        """Send an exact terminal key sequence and record its visible cue.

        Enhanced keyboard mode distinguishes Ctrl+M from Enter with the CSI-u
        sequence below. tmux's named ``C-m`` key is the legacy carriage return,
        so focused recordings must send the negotiated bytes literally.
        """

        run(["tmux", "send-keys", "-t", self.session, "-l", sequence])
        self.remember(label, window)

    def type_text(self, value: str, label: str, window: str, delay: float = 0.024) -> None:
        for character in value:
            run(["tmux", "send-keys", "-t", self.session, "-l", character])
            time.sleep(delay)
        self.remember(label, window)

    def type_line(self, value: str, label: str, window: str, delay: float = 0.024) -> None:
        self.type_text(value, label, window, delay)
        self.key("Enter", "Enter", window)

    def finish(self) -> None:
        if self.closed:
            return
        self.closed = True
        run(["tmux", "send-keys", "-t", self.session, "C-c"], check=False)
        time.sleep(0.25)
        run(["tmux", "send-keys", "-t", self.session, "-l", "exit"], check=False)
        run(["tmux", "send-keys", "-t", self.session, "Enter"], check=False)
        time.sleep(0.8)
        run(["tmux", "kill-session", "-t", self.session], check=False)


def base_environment(root: Path) -> dict[str, str]:
    directories = {
        "HOME": root / "home",
        "XDG_CONFIG_HOME": root / "config",
        "XDG_CACHE_HOME": root / "cache",
        "XDG_DATA_HOME": root / "data",
        "XDG_STATE_HOME": root / "state",
        "TMPDIR": root / "tmp",
    }
    bin_dir = root / "home" / ".local" / "bin"
    for directory in (*directories.values(), bin_dir, root / "home" / "work" / "acme-dashboard"):
        directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    demo_instructions = """This is a disposable Open Agent View demo workspace.
Answer conversational questions directly in at most two short sentences.
Do not call tools, inspect files, or modify the workspace unless explicitly asked.
"""
    for instructions_file in ("AGENTS.md", "CLAUDE.md"):
        (root / "home" / "work" / "acme-dashboard" / instructions_file).write_text(
            demo_instructions,
            encoding="utf-8",
        )
    # Keep provider discovery isolated from the host PATH. Only recording
    # prerequisites are linked in; provider executables are added explicitly
    # by expose_installed_providers or by their real setup installer.
    for tool in ("gh", "uv"):
        if executable := shutil.which(tool):
            (bin_dir / tool).symlink_to(Path(executable).resolve())
    values = {key: str(value) for key, value in directories.items()}
    values.update(
        {
            "PATH": (
                f"{bin_dir}:{root / 'bin'}:{root / 'home' / '.grok' / 'bin'}:"
                f"{root / 'home' / '.kimi-code' / 'bin'}:"
                "/usr/local/pkgs/bin:/usr/local/bin:/usr/bin:/bin"
            ),
            # The OSC marker is invisible in the terminal, but lets the driver
            # prove that asciinema's inner shell—not tmux's outer shell—is ready.
            "PS1": "$ \\[\\e]777;OAV-DEMO-READY\\a\\]",
            "TERM": "xterm-256color",
            "OAV_INSTALL_DIR": str(bin_dir),
            "MUSE_INSTALL_DIR": str(bin_dir),
            "MUSE_NO_MODIFY_PATH": "1",
            "QWEN_INSTALL_BIN_DIR": str(bin_dir),
            "QWEN_NO_MODIFY_PATH": "1",
            "NPM_CONFIG_PREFIX": str(root / "home" / ".local"),
            "KIMI_INSTALL_DIR": str(root / "home" / ".kimi-code"),
            "KIMI_NO_MODIFY_PATH": "1",
            # The shared CI/user account can legitimately exhaust Linux's
            # per-user inotify-instance pool. Keep Kimi's disposable watcher
            # functional without touching host processes or kernel limits.
            "CHOKIDAR_USEPOLLING": "1",
            "CHOKIDAR_INTERVAL": "1000",
            "UV_TOOL_BIN_DIR": str(bin_dir),
            "UV_TOOL_DIR": str(root / "data" / "uv" / "tools"),
            # Provider login flows may consult GitHub CLI state. Point at the
            # existing config without copying credentials into the demo home.
            "GH_CONFIG_DIR": os.environ.get(
                "GH_CONFIG_DIR", str(Path.home() / ".config" / "gh")
            ),
        }
    )
    for key in ("LANG", "LC_ALL", "LC_CTYPE", "SSL_CERT_FILE", "SSL_CERT_DIR"):
        if value := os.environ.get(key):
            values[key] = value
    return values


def expose_installed_providers(root: Path, required: tuple[str, ...]) -> None:
    """Expose only the real CLIs needed by this isolated recording."""

    candidates = {
        "claude": [Path.home() / ".local/bin/claude"],
        "codex": [Path.home() / ".npm-global/bin/codex", Path.home() / ".local/bin/codex"],
        "pi": [Path.home() / ".local/bin/pi", Path.home() / ".npm-global/bin/pi"],
        "opencode": [Path.home() / ".opencode/bin/opencode", Path.home() / ".local/bin/opencode"],
        "cursor-agent": [Path.home() / ".local/bin/cursor-agent"],
        "copilot": [Path.home() / ".npm-global/bin/copilot", Path.home() / ".local/bin/copilot"],
        "agy": [Path.home() / ".local/bin/agy"],
        "vibe": [Path.home() / ".local/bin/vibe"],
        "vibe-app-server": [Path.home() / ".local/bin/vibe-app-server"],
        "muse": [Path.home() / ".local/bin/muse"],
        "qwen": [
            Path.home() / ".local/bin/qwen",
            Path.home() / ".npm-global/bin/qwen",
        ],
        "kimi": [Path.home() / ".local/bin/kimi"],
        "omp": [Path.home() / ".local/bin/omp"],
        "grok": [Path.home() / ".grok/bin/grok", Path.home() / ".local/bin/grok"],
        "kilo": [Path.home() / ".local/bin/kilo", Path.home() / ".npm-global/bin/kilo"],
        "openhands": [Path.home() / ".local/bin/openhands"],
    }
    destination = root / "home" / ".local" / "bin"
    for name in required:
        paths = candidates[name]
        resolved = shutil.which(name)
        source = Path(resolved) if resolved else next(
            (path for path in paths if path.is_file() and os.access(path, os.X_OK)),
            None,
        )
        if source is None:
            raise RuntimeError(f"real demo requires installed provider executable: {name}")
        (destination / name).symlink_to(source.resolve())


def prepare_complete_picker(root: Path, environment: dict[str, str]) -> None:
    """Populate the disposable PATH with genuine CLIs for the setup picker.

    Section 1 proves the released OAV install and its complete picker. It does
    not exercise provider authentication, so the recorder exposes installed
    host executables without copying their state and installs only the missing
    official provider CLIs into the disposable home before recording begins.
    """

    expose_installed_providers(
        root,
        (
            "claude",
            "codex",
            "pi",
            "opencode",
            "cursor-agent",
            "copilot",
            "agy",
            "muse",
            "openhands",
        ),
    )
    script_installers = (
        (
            "Mistral Vibe",
            "https://mistral.ai/vibe/install.sh",
            ("vibe", "vibe-app-server"),
        ),
        (
            "Qwen Code",
            "https://qwen-code-assets.oss-cn-hangzhou.aliyuncs.com/installation/install-qwen-standalone.sh",
            ("qwen",),
        ),
        (
            "Kimi Code",
            "https://code.kimi.com/kimi-code/install.sh",
            ("kimi",),
        ),
        (
            "Oh My Pi",
            "https://omp.sh/install",
            ("omp",),
        ),
        (
            "Grok",
            "https://x.ai/cli/install.sh",
            ("grok",),
        ),
        (
            "OpenHands",
            "https://install.openhands.dev/install.sh",
            ("openhands",),
        ),
    )
    for label, url, executables in script_installers:
        search_path = environment["PATH"].split(os.pathsep)
        if all(
            any(
                (Path(directory) / executable).is_file()
                and os.access(Path(directory) / executable, os.X_OK)
                for directory in search_path
            )
            for executable in executables
        ):
            continue
        script = root / "tmp" / f"picker-{executables[0]}-install.sh"
        download = subprocess.run(
            [
                "curl",
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                url,
                "--output",
                str(script),
            ],
            check=False,
            capture_output=True,
            text=True,
            timeout=120,
        )
        if download.returncode != 0:
            raise RuntimeError(
                f"failed to download official {label} installer: "
                f"{download.stderr[-1000:]}"
            )
        installed = subprocess.run(
            ["bash", str(script)],
            check=False,
            capture_output=True,
            text=True,
            timeout=300,
            env=environment,
        )
        if installed.returncode != 0:
            detail = (installed.stderr or installed.stdout)[-1500:]
            raise RuntimeError(f"official {label} installer failed: {detail}")
        search_path = environment["PATH"].split(os.pathsep)
        for executable in executables:
            if not any(
                (Path(directory) / executable).is_file()
                and os.access(Path(directory) / executable, os.X_OK)
                for directory in search_path
            ):
                raise RuntimeError(
                    f"official {label} installer did not expose {executable} in the disposable PATH"
                )

    kilo_install = subprocess.run(
        ["npm", "install", "--global", "@kilocode/cli"],
        check=False,
        capture_output=True,
        text=True,
        timeout=300,
        env=environment,
    )
    if kilo_install.returncode != 0:
        detail = (kilo_install.stderr or kilo_install.stdout)[-1500:]
        raise RuntimeError(f"official Kilo Code npm install failed: {detail}")
    kilo = root / "home" / ".local" / "bin" / "kilo"
    if not kilo.is_file() or not os.access(kilo, os.X_OK):
        raise RuntimeError(
            "official Kilo Code npm package did not expose kilo in the disposable PATH"
        )


def validate_public_cast(path: Path, required: list[str]) -> None:
    visible = visible_cast(path)
    compact = re.sub(r"\s+", "", visible)
    for value in required:
        if value not in visible and re.sub(r"\s+", "", value) not in compact:
            raise RuntimeError(f"real cast {path.name} is missing {value!r}")
    raw = path.read_text(encoding="utf-8")
    if SECRET_PATTERN.search(raw) or EMAIL_PATTERN.search(raw):
        raise RuntimeError(f"refusing to publish credential-like text in {path.name}")
    if DEVICE_CODE_PATTERN.search(raw):
        raise RuntimeError(f"refusing to publish a one-time device code in {path.name}")
    for value in RECORDER_SECRET_VALUES:
        if value and value in raw:
            raise RuntimeError(f"refusing to publish recorder credentials in {path.name}")
    hostname = socket.gethostname()
    for identity in (os.environ.get("USER", ""), hostname, hostname.split(".", 1)[0]):
        if identity and identity in raw:
            raise RuntimeError(f"refusing to publish host identity in {path.name}")


def public_path_replacements(root: Path) -> dict[str, str]:
    replacements = {
        str(root / "home"): "~",
        str(root): "/tmp/oav-demo",
        str(Path.home()): "~",
        "/tmp/AGENTS.md": "<parent>/AGENTS.md",
    }
    if username := os.environ.get("USER"):
        replacements[username] = "demo"
    hostname = socket.gethostname()
    replacements[hostname] = "local"
    replacements[hostname.split(".", 1)[0]] = "local"
    return replacements


def capture_setup(repo: Path, output: Path) -> None:
    root = Path(tempfile.mkdtemp(prefix="oav-real-setup."))
    terminal: RealTerminal | None = None
    try:
        environment = base_environment(root)
        prepare_complete_picker(root, environment)
        terminal = RealTerminal("setup", root, environment)
        terminal.type_line(INSTALL_COMMAND, "Enter · install", "Terminal", 0.012)
        terminal.wait_for(r"installed shorthand:\s*oav", 120)
        time.sleep(0.6)
        terminal.type_line("oav", "Enter · launch oav", "Terminal", 0.08)
        terminal.wait_for(APP_HEADER_PATTERN, 45)
        time.sleep(1.0)
        terminal.type_line("/harness", "Type /harness", "open-agent-view", 0.08)
        terminal.wait_for(r"choose harness", 20)
        terminal.key("Down", "↓ · highlight Codex", "open-agent-view")
        terminal.key("Up", "↑ · highlight Claude", "open-agent-view")
        time.sleep(1.0)
        end = terminal.timeline_time()
        terminal.key("Escape", "Esc · close picker", "open-agent-view")
        terminal.key("Escape", "Esc · quit", "open-agent-view")
        terminal.finish()
        target = output / "setup.cast"
        write_trimmed_cast(
            terminal.raw_cast,
            target,
            0.0,
            end,
            public_path_replacements(root),
        )
        actions = output / "setup.actions.json"
        actions.write_text(
            json.dumps(
                {
                    "duration": end + 1.35,
                    "actions": [
                        item for item in terminal.actions if float(item["at"]) <= end + 1.35
                    ],
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        actions.chmod(0o644)
        validate_public_cast(
            target,
            [
                INSTALL_COMMAND,
                APP_HEADER,
                "choose harness",
                "Claude",
                "Codex",
                "Pi",
                "OpenCode",
                "Cursor",
                "GitHub Copilot",
                "Antigravity",
                "Mistral Vibe",
                "Muse Code",
                "Qwen Code",
                "Kimi Code",
                "Oh My Pi",
                "Grok",
                "Kilo Code",
                "OpenHands",
                "Terminal",
            ],
        )
        print(f"captured real installer and Open Agent View TUI: {target}")
    finally:
        if terminal is not None:
            terminal.finish()
        terminate_owned_processes(root)
        shutil.rmtree(root, ignore_errors=True)


def private_copy(source: Path, target: Path) -> None:
    if not source.is_file():
        raise RuntimeError(f"required authenticated provider state is missing: {source}")
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    shutil.copyfile(source, target)
    target.chmod(stat.S_IRUSR | stat.S_IWUSR)


def optional_private_copy(source: Path, target: Path) -> None:
    if source.is_file():
        private_copy(source, target)


def prepare_provider_login(provider: str, root: Path, environment: dict[str, str]) -> None:
    """Make a short-lived private copy of only the provider's login/config files."""

    home = root / "home"
    if provider == "codex":
        codex_home = home / ".codex"
        private_copy(Path.home() / ".codex" / "auth.json", codex_home / "auth.json")
        optional_private_copy(Path.home() / ".codex" / "models_cache.json", codex_home / "models_cache.json")
        environment["CODEX_HOME"] = str(codex_home)
    elif provider == "pi":
        source = Path.home() / ".pi" / "agent"
        target = home / ".pi" / "agent"
        private_copy(source / "auth.json", target / "auth.json")
        optional_private_copy(source / "models-store.json", target / "models-store.json")
        optional_private_copy(source / "settings.json", target / "settings.json")
    elif provider == "opencode":
        private_copy(
            Path.home() / ".local" / "share" / "opencode" / "auth.json",
            root / "data" / "opencode" / "auth.json",
        )
    elif provider == "cursor":
        cursor_auth = Path.home() / ".config" / "cursor" / "auth.json"
        private_copy(cursor_auth, root / "config" / "cursor" / "auth.json")
        # Cursor currently follows HOME rather than XDG_CONFIG_HOME for this
        # file, while older builds used the XDG location. Keep both private.
        private_copy(cursor_auth, home / ".config" / "cursor" / "auth.json")
        optional_private_copy(
            Path.home() / ".cursor" / "cli-config.json",
            home / ".cursor" / "cli-config.json",
        )
    elif provider == "copilot":
        private_copy(Path.home() / ".copilot" / "config.json", home / ".copilot" / "config.json")
    elif provider == "antigravity":
        source = Path.home() / ".gemini"
        target = home / ".gemini"
        for relative in (
            "gemini-credentials.json",
            "google_accounts.json",
            "settings.json",
            "state.json",
            "antigravity-cli/antigravity-oauth-token",
            "antigravity-cli/jetski_state.pbtxt",
            "antigravity-cli/cache/onboarding.json",
            "config/config.json",
        ):
            optional_private_copy(source / relative, target / relative)
        antigravity_settings = target / "antigravity-cli" / "settings.json"
        antigravity_settings.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        antigravity_settings.write_text(
            json.dumps(
                {
                    "trustedWorkspaces": [
                        str(home / "work" / "acme-dashboard")
                    ]
                }
            )
            + "\n",
            encoding="utf-8",
        )
        antigravity_settings.chmod(stat.S_IRUSR | stat.S_IWUSR)


def write_private_text(path: Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    path.write_text(value, encoding="utf-8")
    path.chmod(stat.S_IRUSR | stat.S_IWUSR)


def require_host_secret(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise RuntimeError(
            f"the authenticated real demo requires {name} in the recorder environment"
        )
    RECORDER_SECRET_VALUES.add(value)
    return value


def prepare_session_migrate_provider_state(
    provider: str,
    root: Path,
    environment: dict[str, str],
    openai_key: str,
) -> None:
    """Configure one extended harness in the disposable recorder home."""

    home = root / "home"
    if provider == "omp":
        environment["PI_CODING_AGENT_DIR"] = str(home / ".omp" / "agent")
        write_private_text(
            home / ".omp" / "agent" / "config.yml",
            "setupVersion: 2\n",
        )
    elif provider == "grok":
        grok_home = home / ".grok"
        environment["GROK_HOME"] = str(grok_home)
        environment["GROK_DISABLE_AUTOUPDATER"] = "1"
        write_private_text(
            grok_home / "config.toml",
            '''[cli]
auto_update = false

[models]
default = "gpt-5.4"

[features]
telemetry = false
feedback = false

[model."gpt-5.4"]
model = "gpt-5.4"
base_url = "https://api.openai.com/v1"
name = "GPT-5.4"
env_key = "OPENAI_API_KEY"
api_backend = "responses"
context_window = 1047576
''',
        )
    elif provider == "kilo":
        write_private_text(
            root / "config" / "kilo" / "kilo.jsonc",
            json.dumps(
                {
                    "$schema": "https://app.kilo.ai/config.json",
                    "model": "openai/gpt-4.1",
                    "provider": {
                        "openai": {
                            "options": {"apiKey": "{env:OPENAI_API_KEY}"},
                        }
                    },
                }
            )
            + "\n",
        )
        # Kilo's interactive TUI resolves direct-provider credentials from its
        # auth store. Seed the disposable store in the same schema written by
        # `kilo auth login`; it is private, deleted after capture, and never
        # copied into a public cast.
        write_private_text(
            root / "data" / "kilo" / "auth.json",
            json.dumps({"openai": {"type": "api", "key": openai_key}}) + "\n",
        )
    elif provider == "openhands":
        environment["OPENHANDS_SUPPRESS_BANNER"] = "1"
        environment["LLM_API_KEY"] = openai_key
        environment["LLM_MODEL"] = "gpt-4.1"
    else:
        raise RuntimeError(f"unsupported extended demo provider: {provider}")


def prepare_sequence_provider_state(
    root: Path,
    environment: dict[str, str],
) -> None:
    """Prepare every provider before recording, without showing setup in the cast."""

    home = root / "home"
    work = home / "work" / "acme-dashboard"

    claude_config = root / "claude-config"
    private_copy(
        Path.home() / ".claude" / ".credentials.json",
        claude_config / ".credentials.json",
    )
    private_copy(Path.home() / ".claude.json", claude_config / ".claude.json")
    environment.update(
        {
            "CLAUDE_CONFIG_DIR": str(claude_config),
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
            "DISABLE_AUTOUPDATER": "1",
        }
    )

    for provider in (
        "codex",
        "pi",
        "opencode",
        "cursor",
        "copilot",
        "antigravity",
    ):
        prepare_provider_login(provider, root, environment)

    muse_config = root / "config" / "muse"
    private_copy(Path.home() / ".config" / "muse" / "settings.json", muse_config / "settings.json")
    write_private_text(
        muse_config / "trust.json",
        json.dumps(
            {
                "schema_version": 1,
                "projects": {str(work): {"decision": "trusted"}},
            }
        )
        + "\n",
    )
    source_catalog = Path.home() / ".local" / "share" / "muse" / "model-catalog"
    catalog_files = sorted(source_catalog.glob("*.json"))
    if not catalog_files:
        raise RuntimeError("the authenticated Muse model catalog is missing")
    for source in catalog_files:
        private_copy(source, root / "data" / "muse" / "model-catalog" / source.name)

    openai_key = require_host_secret("OPENAI_API_KEY")
    environment["OPENAI_API_KEY"] = openai_key
    vibe_home = home / ".vibe"
    write_private_text(
        vibe_home / "config.toml",
        """active_model = "gpt41-openai"
enable_otel = false

[[providers]]
name = "openai"
api_base = "https://api.openai.com/v1"
api_key_env_var = "OPENAI_API_KEY"
backend = "generic"
emits_finish_reason = true

[[models]]
name = "gpt-4.1"
provider = "openai"
alias = "gpt41-openai"
auto_compact_threshold = 131072
""",
    )
    write_private_text(vibe_home / ".env", f"OPENAI_API_KEY={openai_key}\n")
    write_private_text(
        vibe_home / "trusted_folders.toml",
        f"trusted = [{json.dumps(str(work.resolve()))}]\nuntrusted = []\n",
    )

    write_private_text(
        home / ".qwen" / "settings.json",
        json.dumps(
            {
                "modelProviders": {
                    "openai": [
                        {
                            "id": "gpt-4.1",
                            "name": "GPT-4.1",
                            "envKey": "OPENAI_API_KEY",
                            "baseUrl": "https://api.openai.com/v1",
                        }
                    ]
                },
                "env": {"OPENAI_API_KEY": openai_key},
                "security": {"auth": {"selectedType": "openai"}},
                "model": {"name": "gpt-4.1"},
            }
        )
        + "\n",
    )

    environment["KIMI_CODE_HOME"] = str(home / ".kimi-code")
    environment["KIMI_DISABLE_TELEMETRY"] = "1"
    write_private_text(
        home / ".kimi-code" / "config.toml",
        f'''default_model = "gpt-5.4"

[providers.openai]
type = "openai_responses"
base_url = "https://api.openai.com/v1"
api_key = {json.dumps(openai_key)}

[models."gpt-5.4"]
provider = "openai"
model = "gpt-5.4"
display_name = "GPT-5.4"
max_context_size = 272000
        capabilities = ["image_in", "thinking"]
''',
    )
    normalized_work = str(work.resolve()).replace("\\", "/").rstrip("/")
    work_slug = re.sub(r"[^a-z0-9._-]+", "-", work.name.lower()).strip("-")
    work_slug = (work_slug[:40].strip("-") or "workspace")
    work_hash = hashlib.sha256(normalized_work.encode()).hexdigest()[:12]
    write_private_text(
        home / ".kimi-code" / "workspace-trust" / f"wd_{work_slug}_{work_hash}",
        json.dumps(
            {
                "root": normalized_work,
                "trustedAt": int(time.time() * 1000),
            }
        ),
    )

    # The four newest integrations use the same disposable OpenAI credential,
    # but still run their genuine native TUIs and persistence layers. None of
    # this private provider state is copied into the public cast.
    for provider in ("omp", "grok", "kilo", "openhands"):
        prepare_session_migrate_provider_state(
            provider,
            root,
            environment,
            openai_key,
        )


@dataclass(frozen=True)
class ProviderDemo:
    id: str
    label: str
    cli_value: str
    ready_pattern: str
    model: str | None = None
    setup_only: bool = False


SEQUENCE_DEMOS = (
    ProviderDemo("claude", "Claude Code", "claude", r"Claude Code v", "opus"),
    ProviderDemo("codex", "OpenAI Codex", "codex", r"Codex|OpenAI", "gpt-5.6-sol"),
    ProviderDemo(
        "pi",
        "Pi",
        "pi",
        r"Pi|pi",
        "openai/o1-pro",
    ),
    ProviderDemo(
        "opencode",
        "OpenCode",
        "opencode",
        r"OpenCode|opencode",
        "github-copilot/gpt-5.6-luna",
    ),
    ProviderDemo(
        "cursor",
        "Cursor",
        "cursor",
        r"Cursor|cursor",
        "auto",
    ),
    ProviderDemo(
        "copilot",
        "GitHub Copilot",
        "copilot",
        r"Copilot|copilot",
        "gpt-5.4",
    ),
    ProviderDemo(
        "antigravity",
        "Antigravity",
        "antigravity",
        r"Antigravity|antigravity",
        "gemini-3.7-flash-high",
    ),
    ProviderDemo(
        "mistral-vibe",
        "Mistral Vibe",
        "mistral-vibe",
        r"Mistral Vibe|vibe",
        "gpt41-openai",
    ),
    ProviderDemo(
        "muse",
        "Muse Code",
        "muse",
        r"Muse Code|Muse",
        "meta/muse-spark-1.2",
    ),
    ProviderDemo(
        "qwen",
        "Qwen Code",
        "qwen",
        r"Qwen Code|Qwen",
        "gpt-4.1",
    ),
    ProviderDemo(
        "kimi",
        "Kimi Code",
        "kimi",
        r"Send /help for help information\.",
        "gpt-5.4",
    ),
    ProviderDemo(
        "omp",
        "Oh My Pi",
        "omp",
        r"Oh My Pi|omp v|OMP",
        "openai/gpt-4.1",
    ),
    ProviderDemo(
        "grok",
        "Grok",
        "grok",
        r"Shift\+Tab:mode|GPT-5\.4",
        "gpt-5.4",
    ),
    ProviderDemo(
        "kilo",
        "Kilo Code",
        "kilo",
        r"Kilo Code|kilo",
        "openai/gpt-4.1",
    ),
    ProviderDemo(
        "openhands",
        "OpenHands",
        "openhands",
        r"OpenHands",
        "gpt-4.1",
    ),
    ProviderDemo("terminal", "Terminal", "terminal", r"[$#]\s*$", "bash"),
)


SEQUENCE_ROW_LABELS = {
    "claude": "Claude",
    "codex": "Codex",
    "pi": "Pi",
    "opencode": "OpenCode",
    "cursor": "Cursor",
    "copilot": "GitHub Copilot",
    "antigravity": "Antigravity",
    "mistral-vibe": "Mistral Vibe",
    "muse": "Muse Code",
    "qwen": "Qwen Code",
    "kimi": "Kimi Code",
    "omp": "Oh My Pi",
    "grok": "Grok",
    "kilo": "Kilo Code",
    "openhands": "OpenHands",
    "terminal": "Terminal",
}


PROVIDER_DEMOS = {
    "codex": ProviderDemo("codex", "OpenAI Codex", "codex", r"Codex|OpenAI"),
    "pi": ProviderDemo("pi", "Pi", "pi", r"Pi|pi"),
    "opencode": ProviderDemo(
        "opencode",
        "OpenCode",
        "opencode",
        r"OpenCode|opencode",
        "github-copilot/gpt-5.6-luna",
    ),
    "cursor": ProviderDemo("cursor", "Cursor", "cursor", r"Cursor|cursor", "auto"),
    "copilot": ProviderDemo("copilot", "GitHub Copilot", "copilot", r"Copilot|copilot"),
    "antigravity": ProviderDemo(
        "antigravity",
        "Antigravity",
        "antigravity",
        r"Antigravity|antigravity",
        "gemini-3.7-flash-high",
    ),
    "mistral-vibe": ProviderDemo(
        "mistral-vibe", "Mistral Vibe", "mistral-vibe", r"Mistral Vibe|vibe", setup_only=True
    ),
    "muse": ProviderDemo("muse", "Muse Code", "muse", r"Muse Code|Muse", setup_only=True),
    "qwen": ProviderDemo("qwen", "Qwen Code", "qwen", r"Qwen Code|Qwen", setup_only=True),
    "kimi": ProviderDemo("kimi", "Kimi Code", "kimi", r"Kimi Code|Kimi", setup_only=True),
    "omp": ProviderDemo("omp", "Oh My Pi", "omp", r"Oh My Pi|omp v|OMP"),
    "grok": ProviderDemo("grok", "Grok", "grok", r"Shift\+Tab:mode|GPT-5\.4"),
    "kilo": ProviderDemo("kilo", "Kilo Code", "kilo", r"Kilo Code|kilo"),
    "openhands": ProviderDemo("openhands", "OpenHands", "openhands", r"OpenHands"),
    "terminal": ProviderDemo("terminal", "Terminal", "terminal", r"[$#]\s*$"),
}

CONTROL_DEMOS = ("rename", "switch", "model", "login", "migration")


def provider_disable_flags(active: str) -> list[str]:
    flags = []
    for provider in (
        "claude", "codex", "pi", "opencode", "cursor", "copilot", "antigravity",
        "mistral-vibe", "muse", "qwen", "kimi",
        "omp", "grok", "kilo", "openhands",
    ):
        if provider != active:
            flags.append(f"--no-host-{provider}")
    return flags


def capture_provider_setup(terminal: RealTerminal, spec: ProviderDemo) -> None:
    """Record a real, credential-free provider setup/login flow."""

    terminal.type_line(
        f"/setup {spec.cli_value}",
        f"Type /setup {spec.cli_value}",
        "open-agent-view",
        0.045,
    )
    terminal.wait_screen(rf"Install {re.escape(spec.label)}.*\[y/N\]", 45)
    terminal.key("y", f"Y · install {spec.label}", "Harness setup")
    terminal.key("Enter", "Enter · confirm install", "Harness setup")
    terminal.wait_screen(
        rf"{re.escape(spec.label)} installation completed|interactive login now\?",
        240,
    )
    terminal.wait_screen(r"interactive login now\?", 45)
    terminal.remember("Install check complete", "Harness setup")
    time.sleep(1.8)
    terminal.key("Enter", f"Enter · open {spec.label} login", "Harness setup")

    provider_ready = {
        "mistral-vibe": r"setup|provider|model|API|Mistral|login|sign[ -]?in",
        "muse": r"auth|browser|device|code|login|sign[ -]?in|Muse",
        "qwen": r"Qwen|/auth|auth|theme|workspace|trust",
        "kimi": r"device|code|login|browser|https://|Kimi",
    }[spec.id]
    terminal.wait_screen(provider_ready, 90)
    if spec.id == "qwen":
        # Qwen performs authentication from its native slash-command surface.
        if "Workspace Trust" in terminal.screen():
            terminal.key("a", "A · trust disposable workspace", spec.label)
            time.sleep(0.8)
        terminal.type_line("/auth", "Type /auth", spec.label, 0.08)
        terminal.wait_screen(r"auth|login|browser|device|code", 60)
    terminal.remember("Native login ready", f"{spec.label} login")
    time.sleep(2.8)
    terminal.key("S-Left", "Shift+← · background setup", f"{spec.label} login")
    terminal.wait_screen(APP_HEADER_PATTERN, 30)
    terminal.remember("Returned to dashboard", "open-agent-view")
    time.sleep(2.0)


def write_provider_recording(
    root: Path,
    output: Path,
    terminal: RealTerminal,
    spec: ProviderDemo,
    start: float,
    end: float,
    proof: str,
) -> tuple[Path, Path]:
    target = output / f"{spec.id}.cast"
    write_trimmed_cast(
        terminal.raw_cast,
        target,
        start,
        end,
        public_path_replacements(root),
    )
    actions = [
        {**item, "at": max(0.0, round(float(item["at"]) - start, 3))}
        for item in terminal.actions
        if start <= float(item["at"]) <= end
    ]
    action_path = output / f"{spec.id}.actions.json"
    action_path.write_text(
        json.dumps(
            {"duration": end - start + 1.35, "proof": proof, "actions": actions},
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    action_path.chmod(0o644)
    compact_recording(target, action_path)
    required = [APP_HEADER, spec.label]
    if proof == "conversation":
        required.extend(["choose harness", "One dashboard, every harness.", "Session still here."])
    else:
        required.extend(["Install", "login"])
    validate_public_cast(target, required)
    return target, action_path


def capture_provider(repo: Path, output: Path, spec: ProviderDemo) -> None:
    root = Path(tempfile.mkdtemp(prefix=f"oav-real-{spec.id}."))
    terminal: RealTerminal | None = None
    try:
        environment = base_environment(root)
        executable_names = {
            "codex": ("codex",),
            "pi": ("pi",),
            "opencode": ("opencode",),
            "cursor": ("cursor-agent",),
            "copilot": ("copilot",),
            "antigravity": ("agy",),
            "omp": ("omp",),
            "grok": ("grok",),
            "kilo": ("kilo",),
            "openhands": ("openhands",),
            "terminal": (),
        }
        if not spec.setup_only:
            expose_installed_providers(root, executable_names[spec.id])
            prepare_provider_login(spec.id, root, environment)
            if spec.id in {"omp", "grok", "kilo", "openhands"}:
                openai_key = require_host_secret("OPENAI_API_KEY")
                environment["OPENAI_API_KEY"] = openai_key
                prepare_session_migrate_provider_state(
                    spec.id,
                    root,
                    environment,
                    openai_key,
                )
        binary = repo / "target" / "release" / "open-agent-view"
        if not binary.is_file():
            raise RuntimeError("build target/release/open-agent-view before recording providers")
        bin_dir = root / "home" / ".local" / "bin"
        (bin_dir / "open-agent-view").symlink_to(binary.resolve())
        (bin_dir / "oav").symlink_to("open-agent-view")

        work = root / "home" / "work" / "acme-dashboard"
        terminal = RealTerminal(spec.id, root, environment)
        command = [
            "oav",
            "--cwd",
            str(work),
            "--launch-cwd",
            str(work),
            "--refresh-ms",
            "30000",
            "--harness",
            spec.cli_value,
            *provider_disable_flags(spec.id),
        ]
        terminal.type_line(
            shlex.join(command), "Enter · launch oav", "Terminal", 0.001
        )
        terminal.wait_for(APP_HEADER_PATTERN, 45)
        time.sleep(0.8)
        start = terminal.repaint_start()
        if spec.setup_only:
            capture_provider_setup(terminal, spec)
            end = terminal.timeline_time()
            terminal.key("Escape", "Esc · quit", "open-agent-view")
            terminal.finish()
            write_provider_recording(root, output, terminal, spec, start, end, "setup")
            print(f"captured real Open Agent View → {spec.label} setup TUI")
            return
        terminal.type_line("/harness", "Type /harness", "open-agent-view", 0.07)
        terminal.wait_for(r"choose harness", 20)
        terminal.key("Enter", f"Enter · choose {spec.label}", "open-agent-view")
        time.sleep(0.45)
        if spec.model is not None:
            terminal.type_line(
                f"/model {spec.model}",
                f"Select model · {spec.model}",
                "open-agent-view",
                0.012,
            )
            time.sleep(0.5)

        if spec.id == "terminal":
            terminal.type_line("verification shell", "Enter · open Terminal", "open-agent-view", 0.045)
            terminal.wait_screen(spec.ready_pattern, 30)
            first_command = "printf 'One dashboard, every harness.\\n'"
            terminal.type_line(first_command, "Enter · run command", "Terminal", 0.02)
            terminal.wait_screen(r"One dashboard, every harness\.", 15)
            second_command = "printf 'Session still here.\\n'"
            terminal.type_line(second_command, "Enter · run follow-up", "Terminal", 0.02)
            terminal.wait_screen(r"Session still here\.", 15)
            terminal.remember("Command finished", "Terminal")
        else:
            first_prompt = "Reply exactly: One dashboard, every harness."
            terminal.type_line(
                first_prompt,
                f"Enter · start {spec.label}",
                "open-agent-view",
                0.016,
            )
            terminal.wait_screen(spec.ready_pattern, 75)
            if "Workspace Trust Required" in terminal.screen():
                terminal.key("a", "A · trust disposable demo workspace", spec.label)
                time.sleep(0.7)
            terminal.wait_screen_occurrences("One dashboard, every harness.", timeout=150)
            # Some native TUIs repaint and install terminal key bindings after
            # the answer appears. Wait for that settle, then prove the prompt
            # is visible in the provider's own input before pressing Enter;
            # otherwise a fast recorder can send the whole line during a
            # repaint and the provider legitimately discards it.
            time.sleep(1.5)
            if "Set up terminal for multi-line input support" in terminal.screen():
                terminal.key(
                    "Escape",
                    "Esc · keep terminal settings unchanged",
                    spec.label,
                )
                time.sleep(1.0)
            second_prompt = "Reply exactly: Session still here."
            terminal.type_text(
                second_prompt,
                "Type follow-up",
                spec.label,
                0.016,
            )
            terminal.wait_screen(re.escape(second_prompt), 15)
            terminal.key("Enter", "Enter · send follow-up", spec.label)
            terminal.wait_screen_occurrences("Session still here.", timeout=150)
            terminal.remember("Response ready", spec.label)

        time.sleep(2.2)
        terminal.key("S-Left", "Shift+← · return to oav", spec.label)
        terminal.wait_screen(APP_HEADER_PATTERN, 30)
        time.sleep(1.6)
        terminal.key("Right", f"→ · reopen {spec.label}", "open-agent-view")
        terminal.wait_screen(spec.ready_pattern, 60)
        terminal.remember("Session reopened", spec.label)
        time.sleep(2.2)
        terminal.key("S-Left", "Shift+← · return to oav", spec.label)
        terminal.wait_screen(APP_HEADER_PATTERN, 30)
        time.sleep(1.2)
        end = terminal.timeline_time()

        terminal.key("Escape", "Esc · quit", "open-agent-view")
        terminal.finish()
        target, _ = write_provider_recording(
            root, output, terminal, spec, start, end, "conversation"
        )
        print(f"captured real Open Agent View → {spec.label} TUI: {target}")
    finally:
        if terminal is not None:
            terminal.finish()
        terminate_owned_processes(root)
        shutil.rmtree(root, ignore_errors=True)


SEQUENCE_PLAYBACK_SPEED = 0.5
SEQUENCE_TYPING_SPEEDUP = 0.8
CLAUDE_LAUNCH_TARGET_SECONDS = 1.8
# Section 2 moves from 0.5x to 0.6x playback. Scaling the real Codex Working
# interval by 0.4 therefore makes that state exactly 3x faster than before:
# (source * 0.4 / 0.6) == (source / 0.5 / 3).
CODEX_WORKING_CAST_SCALE = 0.4


def sequence_wait(visible_seconds: float) -> None:
    """Wait for an exact viewer-visible duration at the configured playback speed."""

    time.sleep(visible_seconds * SEQUENCE_PLAYBACK_SPEED)


def sequence_type_text(
    terminal: RealTerminal,
    value: str,
    label: str,
    window: str,
    delay: float,
) -> None:
    terminal.type_text(value, label, window, delay * SEQUENCE_TYPING_SPEEDUP)


def sequence_type_line(
    terminal: RealTerminal,
    value: str,
    label: str,
    window: str,
    delay: float,
) -> None:
    sequence_type_text(terminal, value, label, window, delay)
    sequence_wait(1.0)
    terminal.key("Enter", "Enter", window)


def select_sequence_model(terminal: RealTerminal, spec: ProviderDemo) -> None:
    option = "shell" if spec.id == "terminal" else "model"
    sequence_type_line(
        terminal,
        f"/{option}",
        f"Type /{option}",
        "open-agent-view",
        0.055,
    )
    picker_pattern = rf"choose .* {option}"
    terminal.wait_screen(picker_pattern, 60)
    terminal.wait_screen_without(
        r"Loading models…|Finding installed shells…",
        120,
    )
    model_screen = terminal.screen()
    if re.search(
        r"failed to list|model discovery exited|not authenticated|no models",
        model_screen,
        re.IGNORECASE,
    ) and "Use exact model ID" not in model_screen:
        raise RuntimeError(
            f"{spec.label} model discovery failed before the recording could select a model"
        )
    terminal.key("Down", f"↓ · browse {option}s", "open-agent-view")
    sequence_wait(0.675)
    terminal.key("Down", f"↓ · browse {option}s", "open-agent-view")
    sequence_wait(0.675)
    terminal.key("Up", f"↑ · compare {option}s", "open-agent-view")
    sequence_wait(0.975)
    sequence_type_text(
        terminal,
        spec.model,
        f"Search · {spec.model}",
        "open-agent-view",
        0.04,
    )
    terminal.wait_screen_without(r"Loading models…|Finding installed shells…", 120)
    terminal.wait_screen(re.escape(spec.model), 45)
    if not re.search(picker_pattern, terminal.screen(), re.IGNORECASE):
        raise RuntimeError(
            f"{spec.label} model picker closed while searching for {spec.model}"
        )
    if spec.id == "kilo":
        # Kilo exposes both its hosted proxy (`kilo/openai/gpt-4.1`) and the
        # direct provider (`openai/gpt-4.1`). A substring search matches both;
        # move past the three hosted GPT-4.1 variants to the exact direct ID.
        for _ in range(3):
            terminal.key(
                "Down",
                "↓ · choose direct OpenAI model",
                "open-agent-view",
            )
            sequence_wait(0.35)
    sequence_wait(1.5)
    terminal.key("Enter", f"Enter · select {spec.model}", "open-agent-view")
    terminal.wait_screen_without(picker_pattern, 30)
    if spec.id == "kilo":
        terminal.wait_screen(
            rf"harness\s+Kilo Code\s+·\s+model\s+{re.escape(spec.model)}",
            20,
        )
        if "model kilo/openai/gpt-4.1" in terminal.screen():
            raise RuntimeError("Kilo model picker selected its hosted proxy instead of openai/gpt-4.1")
    sequence_wait(1.5)


def run_terminal_sequence_turns(
    terminal: RealTerminal,
    spec: ProviderDemo,
) -> None:
    first_baseline = terminal.screen()
    sequence_type_line(
        terminal,
        "printf 'Hello from Terminal.\\n'",
        "Enter · printf greeting",
        "Terminal",
        0.055,
    )
    terminal.wait_screen(r"Hello from Terminal", 20)
    terminal.wait_screen_settled(
        first_baseline,
        provider=spec.label,
        timeout=30,
        minimum_wait=0.5,
        stable_for=1.5,
    )
    terminal.remember("Command complete", "Terminal")

    explanation = "printf 'Terminal is a real shell managed beside coding agents.\\n'"
    baseline = terminal.screen()
    sequence_type_line(
        terminal,
        explanation,
        "Enter · printf explanation",
        "Terminal",
        0.055,
    )
    terminal.wait_screen(r"Terminal is a real shell", 20)
    terminal.wait_screen_settled(
        baseline,
        provider=spec.label,
        timeout=30,
        minimum_wait=0.5,
        stable_for=1.5,
    )
    terminal.remember("Explanation complete", "Terminal")


def run_native_sequence_turns(terminal: RealTerminal, spec: ProviderDemo) -> None:
    def reject_failed_turn() -> None:
        screen = terminal.screen()
        if re.search(
            r"API Error|Error:\s*4\d\d|HTTP\s+4\d\d|unsupported parameter|"
            r"Agent execution terminated due to error|"
            r"Authentication required|not authenticated|run exited without success|"
            r"Named models unavailable|Free plans can only use Auto|"
            r"You need to sign in to use this model",
            screen,
            re.IGNORECASE,
        ):
            tail = screen[-2400:].replace(str(Path.home()), "~")
            raise RuntimeError(
                f"{spec.label} rendered a provider failure instead of a completed turn: "
                f"{tail!r}"
            )

    baseline = terminal.screen()
    sequence_type_line(
        terminal,
        "hello",
        f"Enter · start {spec.label}",
        "open-agent-view",
        0.085,
    )
    if spec.id == "claude":
        terminal.mark_timing("claude-launch-start")
    terminal.wait_screen_without(APP_HEADER_PATTERN, 90)
    terminal.wait_native_screen(spec.ready_pattern, 90)
    if spec.id == "claude":
        terminal.mark_timing("claude-launch-end")
    if spec.id == "codex":
        terminal.mark_timing("codex-working-1-start")
        terminal.wait_codex_response_started(baseline)
        terminal.mark_timing("codex-working-1-end")
    terminal.wait_screen_settled(
        baseline,
        provider=spec.label,
        minimum_wait=0.75,
        stable_for=1.5,
    )
    reject_failed_turn()
    terminal.remember("Hello response complete", spec.label)

    explanation = f"Explain what is {spec.label}"
    sequence_type_text(
        terminal,
        explanation,
        f"Type · {explanation}",
        spec.label,
        0.055,
    )
    terminal.wait_screen(re.escape(explanation), 30)
    sequence_wait(1.0)
    baseline = terminal.screen()
    terminal.key("Enter", "Enter · send explanation", spec.label)
    if spec.id == "codex":
        terminal.mark_timing("codex-working-2-start")
        terminal.wait_codex_response_started(baseline)
        terminal.mark_timing("codex-working-2-end")
    terminal.wait_screen_settled(
        baseline,
        provider=spec.label,
        minimum_wait=0.75,
        stable_for=1.5,
    )
    reject_failed_turn()
    terminal.remember("Explanation response complete", spec.label)


def run_native_overview_seed(terminal: RealTerminal, spec: ProviderDemo) -> None:
    """Create one genuine, settled provider session outside the overview clip.

    The overview needs a populated dashboard, not a replay of every longer
    Section 2 conversation. A single real greeting proves each provider can
    launch and respond while avoiding unrelated web lookups during pre-roll.
    """

    for attempt in range(1, 4):
        baseline = terminal.screen()
        sequence_type_line(
            terminal,
            "hello",
            f"Enter · prelaunch {spec.label}",
            "open-agent-view",
            0.04,
        )
        deadline = time.monotonic() + 90
        while re.search(APP_HEADER_PATTERN, terminal.screen()):
            screen = terminal.screen()
            if re.search(r"launch failed:", screen, re.IGNORECASE):
                if attempt == 3:
                    tail = screen[-2400:].replace(str(Path.home()), "~")
                    raise RuntimeError(
                        f"{spec.label} failed all three real overview launches: {tail!r}"
                    )
                terminal.remember(
                    f"Retry {spec.label} launch · {attempt + 1}/3",
                    "open-agent-view",
                )
                time.sleep(2.0)
                break
            if time.monotonic() >= deadline:
                raise RuntimeError(f"{spec.label} did not open its native TUI")
            time.sleep(0.1)
        else:
            terminal.wait_native_screen(spec.ready_pattern, 90)
            if spec.id == "codex":
                terminal.wait_codex_response_started(baseline)
            terminal.wait_screen_settled(
                baseline,
                provider=spec.label,
                minimum_wait=0.75,
                stable_for=1.5,
            )
            terminal.key("S-Left", "Shift+← · background seeded session", spec.label)
            terminal.wait_screen(APP_HEADER_PATTERN, 45)
            terminal.remember("Seeded session visible", "open-agent-view")
            sequence_wait(0.75)
            return


def write_sequence_recording(
    root: Path,
    output: Path,
    terminal: RealTerminal,
    spec: ProviderDemo,
    start: float,
    end: float,
    prior_names: list[str],
    action_start_index: int,
) -> None:
    target = output / f"{spec.id}.cast"
    write_trimmed_cast(
        terminal.raw_cast,
        target,
        start,
        end,
        public_path_replacements(root),
    )
    primed_lead = prime_first_terminal_frame(target)
    actions = [
        {
            **item,
            "at": max(
                0.0,
                round(float(item["at"]) - start - primed_lead, 3),
            ),
        }
        for item in terminal.actions[action_start_index:]
        if start <= float(item["at"]) <= end
    ]
    action_path = output / f"{spec.id}.actions.json"
    action_path.write_text(
        json.dumps(
            {
                "duration": end - start + 1.35 - primed_lead,
                "proof": "conversation" if spec.id != "terminal" else "terminal",
                "sequence": "picker-model-two-turns-return-rename-picker",
                "actions": actions,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    action_path.chmod(0o644)
    marks = {
        str(item["label"]): float(item["at"]) - start - primed_lead
        for item in terminal.timing_marks
        if start <= float(item["at"]) <= end
    }
    intervals: list[CompressionInterval] = []
    if spec.id == "claude":
        source_start = marks.get("claude-launch-start")
        source_end = marks.get("claude-launch-end")
        if source_start is None or source_end is None:
            raise RuntimeError("Claude sequence is missing measured launch timing marks")
        source_duration = source_end - source_start
        if source_duration > CLAUDE_LAUNCH_TARGET_SECONDS:
            intervals.append(
                CompressionInterval(
                    "Claude launch/background handoff",
                    source_start,
                    source_end,
                    CLAUDE_LAUNCH_TARGET_SECONDS,
                )
            )
    elif spec.id == "codex":
        for turn in (1, 2):
            source_start = marks.get(f"codex-working-{turn}-start")
            source_end = marks.get(f"codex-working-{turn}-end")
            if source_start is None or source_end is None:
                raise RuntimeError(
                    f"Codex sequence is missing measured Working interval {turn}"
                )
            source_duration = source_end - source_start
            intervals.append(
                CompressionInterval(
                    f"Codex Working turn {turn}",
                    source_start,
                    source_end,
                    source_duration * CODEX_WORKING_CAST_SCALE,
                )
            )
    retime_recording_intervals(target, action_path, intervals)
    validate_public_cast(
        target,
        [
            APP_HEADER,
            "choose harness",
            spec.label,
            "Hello from Terminal" if spec.id == "terminal" else "hello",
            (
                "Terminal is a real shell"
                if spec.id == "terminal"
                # Grok repaints the submitted prompt in cursor-addressed
                # fragments, so its raw asciicast never contains the prompt
                # as one byte-contiguous string. The visible second-turn
                # response is stable proof without relying on Grok's variable
                # auto-generated conversation title.
                else (
                    "Grok is"
                    if spec.id == "grok"
                    else f"Explain what is {spec.label}"
                )
            ),
            f"{spec.id}-explanation",
            *prior_names,
        ],
    )


def capture_overview_story(
    root: Path,
    output: Path,
    terminal: RealTerminal,
    session_names: list[str],
    target_spec: ProviderDemo,
) -> None:
    """Record the short, genuine dashboard-to-native opening walkthrough."""

    expected_names = [
        f"{spec.id}-explanation" for spec in SEQUENCE_DEMOS
        if spec.id not in {"omp", "grok", "kilo", "openhands", "terminal"}
    ]
    if session_names != expected_names:
        raise RuntimeError(
            "overview requires the eleven completed coding-harness sessions; "
            f"got {session_names!r}"
        )

    terminal.wait_screen_without(r"choose harness", 30)
    terminal.wait_selected_row(target_spec.label, 20)
    for name in session_names:
        if name == "qwen-explanation":
            terminal.wait_screen(r"qwen-explanation|Explain what is Qwen Code", 20)
        else:
            terminal.wait_screen(re.escape(name), 20)
    counts = re.search(
        r"(\d+) awaiting input · (\d+) working · (\d+) completed",
        terminal.screen(),
    )
    if counts is None or sum(int(value) for value in counts.groups()) != 11:
        raise RuntimeError("overview dashboard does not contain exactly eleven sessions")

    action_start_index = len(terminal.actions)
    start = terminal.repaint_start()
    pacing_scale = 1.25
    terminal.remember("Dashboard · 11 coding harnesses", "open-agent-view")
    time.sleep(1.0 * pacing_scale)

    preview_specs = [
        next(spec for spec in SEQUENCE_DEMOS if spec.id == preview_id)
        for preview_id in ("qwen", "muse")
    ]
    for preview_spec in preview_specs:
        terminal.key(
            "Down",
            f"↓ · choose {preview_spec.label}",
            "open-agent-view",
        )
        terminal.wait_selected_row(preview_spec.label, 20)
        time.sleep(0.5 * pacing_scale)
        terminal.key(
            "Right",
            f"→ · open {preview_spec.label}",
            "open-agent-view",
        )
        terminal.wait_screen_without(APP_HEADER_PATTERN, 45)
        terminal.wait_native_screen(preview_spec.ready_pattern, 45)
        terminal.remember(
            f"{preview_spec.label} · native session",
            preview_spec.label,
        )
        # Let the real provider TUI remain readable before returning to the
        # same selected dashboard row.
        time.sleep(2.0)
        terminal.key(
            "S-Left",
            "Shift+← · return to dashboard",
            preview_spec.label,
        )
        terminal.wait_screen(APP_HEADER_PATTERN, 45)
        terminal.wait_selected_row(preview_spec.label, 20)
        terminal.remember(
            f"Dashboard · {preview_spec.label} stays available",
            "open-agent-view",
        )
        time.sleep(0.5 * pacing_scale)

    for _ in range(2):
        terminal.key("Up", "↑ · choose Kimi Code", "open-agent-view")
        time.sleep(0.5 * pacing_scale)
    terminal.wait_selected_row(target_spec.label, 20)

    terminal.key("Right", f"→ · open {target_spec.label}", "open-agent-view")
    terminal.wait_screen_without(APP_HEADER_PATTERN, 45)
    terminal.wait_native_screen(target_spec.ready_pattern, 45)
    terminal.remember(f"{target_spec.label} · native session", target_spec.label)
    time.sleep(0.6 * pacing_scale)

    prompt = (
        "Look up https://open-agent-view.github.io/ "
        "and tell me what it is about."
    )
    terminal.type_text(
        prompt,
        f"Type · {prompt}",
        target_spec.label,
        0.032 * pacing_scale,
    )
    terminal.wait_screen(re.escape(prompt), 30)
    time.sleep(0.6 * pacing_scale)
    terminal.key("Enter", "Enter · send lookup prompt", target_spec.label)
    # This is intentionally a literal seven-second observation window. The
    # overview ends without waiting for the provider to finish generating.
    time.sleep(7.0)
    terminal.key("S-Left", "Shift+← · return to dashboard", target_spec.label)
    terminal.wait_screen(APP_HEADER_PATTERN, 45)
    terminal.remember(
        f"Dashboard · {target_spec.label} keeps running",
        "open-agent-view",
    )
    time.sleep(2.0)
    end = terminal.timeline_time()

    target = output / "overview.cast"
    write_trimmed_cast(
        terminal.raw_cast,
        target,
        start,
        end,
        public_path_replacements(root),
    )
    primed_lead = prime_first_terminal_frame(target)
    actions = [
        {
            **item,
            "at": max(0.0, round(float(item["at"]) - start - primed_lead, 3)),
        }
        for item in terminal.actions[action_start_index:]
        if start <= float(item["at"]) <= end
    ]
    action_path = output / "overview.actions.json"
    action_path.write_text(
        json.dumps(
            {
                "duration": end - start + 1.35 - primed_lead,
                "proof": "conversation",
                "sequence": "eleven-session-dashboard-preview-two-open-kimi-lookup-return",
                "pacing_scale": pacing_scale,
                "preview_targets": [spec.id for spec in preview_specs],
                "preview_seconds": 2,
                "lookup_seconds": 7,
                "target": target_spec.id,
                "session_count": 11,
                "actions": actions,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    action_path.chmod(0o644)
    validate_public_cast(
        target,
        [
            APP_HEADER,
            *(name for name in session_names if name != "qwen-explanation"),
            "Qwen Code",
            target_spec.label,
            prompt,
        ],
    )
    print("captured real eleven-session overview story", flush=True)


def capture_multi_session_rename(
    root: Path,
    output: Path,
    terminal: RealTerminal,
    previous_names: list[str],
) -> None:
    """Record two genuine agent-session renames on the shared dashboard.

    This runs inside the coherent provider sequence after Claude and Codex
    have both completed real conversations. Keeping the same terminal proves
    that Ctrl+R changes the managed rows users just saw instead of substituting
    a prepared or simulated dashboard.
    """

    if previous_names[-2:] != ["claude-explanation", "codex-explanation"]:
        raise RuntimeError(
            "multi-session rename requires the completed Claude and Codex rows"
        )

    terminal.key("Escape", "Esc · close harness picker", "open-agent-view")
    terminal.wait_screen_without(r"choose harness", 30)
    terminal.key("Escape", "Esc · return to session list", "open-agent-view")
    terminal.wait_selected_row("Codex", 20)
    time.sleep(0.8)

    action_start_index = len(terminal.actions)
    start = terminal.repaint_start()
    time.sleep(1.0)

    rename_steps = (
        ("Codex", "release-audit"),
        ("Claude", "frontend-refactor"),
    )
    for index, (provider, new_name) in enumerate(rename_steps):
        terminal.wait_selected_row(provider, 20)
        terminal.key(
            "C-r",
            f"Ctrl+R · rename {provider} session",
            "open-agent-view",
        )
        terminal.wait_screen(r"rename session", 20)
        time.sleep(0.7)
        terminal.key(
            "C-u",
            f"Ctrl+U · clear {provider} name",
            "open-agent-view",
        )
        terminal.type_text(
            new_name,
            f"Type · {new_name}",
            "open-agent-view",
            0.065,
        )
        time.sleep(0.75)
        terminal.key(
            "Enter",
            f"Enter · save {provider} name",
            "open-agent-view",
        )
        terminal.wait_screen(
            rf"(?m)^\s*[^\n]*\b{re.escape(new_name)}\b"
            rf"[^\n]*\b{re.escape(provider)}\b",
            30,
        )
        terminal.wait_selected_row(provider, 20)
        terminal.remember(f"Saved {provider} name", "open-agent-view")
        time.sleep(1.5)
        if index == 0:
            terminal.key(
                "Down",
                "↓ · select Claude session",
                "open-agent-view",
            )
            terminal.wait_selected_row("Claude", 20)
            time.sleep(0.8)

    terminal.wait_screen(
        r"(?m)^\s*[^\n]*\bfrontend-refactor\b[^\n]*\bClaude\b",
        30,
    )
    terminal.wait_screen(
        r"(?m)^\s*[^\n]*\brelease-audit\b[^\n]*\bCodex\b",
        30,
    )
    terminal.remember("Both agent names visible", "open-agent-view")
    time.sleep(2.0)
    end = terminal.timeline_time()

    target = output / "rename.cast"
    write_trimmed_cast(
        terminal.raw_cast,
        target,
        start,
        end,
        public_path_replacements(root),
    )
    primed_lead = prime_first_terminal_frame(target)
    actions = [
        {
            **item,
            "at": max(0.0, round(float(item["at"]) - start - primed_lead, 3)),
        }
        for item in terminal.actions[action_start_index:]
        if start <= float(item["at"]) <= end
    ]
    action_path = output / "rename.actions.json"
    action_path.write_text(
        json.dumps(
            {
                "duration": end - start + 1.35 - primed_lead,
                "proof": "multiple-native-agent-sessions",
                "sequence": "rename-codex-then-claude",
                "actions": actions,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    action_path.chmod(0o644)
    validate_public_cast(
        target,
        [
            APP_HEADER,
            "rename session",
            "claude-explanation",
            "codex-explanation",
            "frontend-refactor",
            "release-audit",
            "Claude",
            "Codex",
        ],
    )

    previous_names[-2:] = ["frontend-refactor", "release-audit"]
    print("captured real multi-session Claude + Codex rename story", flush=True)

    # Restore the picker so a full sequence can continue to Pi and later
    # harnesses, and so the existing recorder cleanup path stays deterministic.
    sequence_type_line(
        terminal,
        "/harness",
        "Type /harness",
        "open-agent-view",
        0.07,
    )
    terminal.wait_screen(r"choose harness", 30)
    sequence_wait(1.0)


def capture_provider_sequence(
    repo: Path,
    output: Path,
    *,
    through: str | None = None,
    capture_rename: bool = False,
    capture_overview: bool = False,
    write_stories: bool = True,
) -> None:
    # Provider supervisors use Unix sockets. Keep the private root short so
    # every provider stays comfortably below sockaddr_un path limits.
    root = Path(tempfile.mkdtemp(prefix="oavseq."))
    terminal: RealTerminal | None = None
    try:
        environment = base_environment(root)
        prepare_complete_picker(root, environment)
        prepare_sequence_provider_state(root, environment)
        install_local_binary(repo, root)
        prepare_cursor_demo_wrapper(root)
        prewarm_sequence_harnesses(root, environment)

        work = root / "home" / "work" / "acme-dashboard"
        terminal = RealTerminal("harness-sequence", root, environment)
        command = [
            "oav",
            "--cwd",
            str(work),
            "--launch-cwd",
            str(work),
            "--refresh-ms",
            "3600000",
            "--harness",
            "claude",
        ]
        sequence_type_line(
            terminal,
            shlex.join(command),
            "Enter · launch oav",
            "Terminal",
            0.001,
        )
        terminal.wait_screen(APP_HEADER_PATTERN, 60)
        sequence_wait(0.75)
        sequence_type_line(
            terminal,
            "/harness",
            "Type /harness",
            "open-agent-view",
            0.07,
        )
        terminal.wait_screen(r"choose harness", 30)
        sequence_wait(0.75)

        previous_names: list[str] = []
        for index, spec in enumerate(SEQUENCE_DEMOS):
            action_start_index = len(terminal.actions)
            start = terminal.repaint_start()
            print(f"recording coherent {spec.label} story…", flush=True)
            picker_search = {
                "claude": "Claude",
                "codex": "Codex",
            }.get(spec.id, spec.label)
            if index > 0:
                terminal.key(
                    "Down",
                    f"↓ · highlight {spec.label}",
                    "open-agent-view",
                )
                sequence_wait(1.2)
            terminal.wait_screen(re.escape(picker_search), 30)
            terminal.key("Enter", f"Enter · choose {spec.label}", "open-agent-view")
            terminal.wait_screen_without(r"choose harness", 30)
            terminal.wait_screen(rf"harness\s+{re.escape(picker_search)}", 30)
            sequence_wait(1.5)

            select_sequence_model(terminal, spec)
            needs_native_return = True
            if spec.id == "terminal":
                sequence_type_line(
                    terminal,
                    "terminal workspace",
                    "Enter · open Terminal",
                    "open-agent-view",
                    0.08,
                )
                terminal.wait_screen_without(APP_HEADER_PATTERN, 45)
                terminal.wait_screen(spec.ready_pattern, 30)
                run_terminal_sequence_turns(terminal, spec)
            else:
                try:
                    if capture_overview:
                        run_native_overview_seed(terminal, spec)
                        needs_native_return = False
                    else:
                        run_native_sequence_turns(terminal, spec)
                except RuntimeError as error:
                    row_label = SEQUENCE_ROW_LABELS[spec.id]
                    screen = terminal.screen()
                    recovered_native = False
                    if (
                        not capture_overview
                        and spec.id == "opencode"
                        and re.search(APP_HEADER_PATTERN, screen)
                        and "No such file or directory" in screen
                    ):
                        # OpenCode can finish its first async turn before the
                        # native attach target becomes visible. Remove that
                        # disposable row and restart this story from a freshly
                        # repainted picker so the published cast contains
                        # neither the transient error nor a phantom session.
                        terminal.wait_selected_row(row_label)
                        terminal.key(
                            "C-x",
                            "Ctrl+X · remove transient OpenCode row",
                            "open-agent-view",
                        )
                        sequence_wait(1.0)
                        sequence_type_line(
                            terminal,
                            "/harness",
                            "Type /harness",
                            "open-agent-view",
                            0.07,
                        )
                        terminal.wait_screen(r"choose harness", 30)
                        action_start_index = len(terminal.actions)
                        start = terminal.repaint_start()
                        terminal.wait_screen(re.escape(picker_search), 30)
                        terminal.key(
                            "Enter",
                            f"Enter · choose {spec.label}",
                            "open-agent-view",
                        )
                        terminal.wait_screen_without(r"choose harness", 30)
                        terminal.wait_screen(
                            rf"harness\s+{re.escape(picker_search)}",
                            30,
                        )
                        sequence_wait(1.5)
                        select_sequence_model(terminal, spec)
                        run_native_sequence_turns(terminal, spec)
                        screen = terminal.screen()
                        recovered_native = True
                    if recovered_native:
                        pass
                    elif not (
                        capture_overview
                        and re.search(APP_HEADER_PATTERN, screen)
                        and row_label in screen
                    ):
                        raise error
                    else:
                        terminal.remember(
                            f"{spec.label} session created; native attach unavailable",
                            "open-agent-view",
                        )
                        sequence_wait(1.0)
                        needs_native_return = False

            if needs_native_return:
                return_draft = "Now, I can navigate back to panel with Shift + Left Arrow"
                sequence_type_text(
                    terminal,
                    return_draft,
                    "Type · explain return shortcut",
                    spec.label,
                    0.04,
                )
                terminal.wait_screen(re.escape(return_draft), 30)
                sequence_wait(1.5)
                terminal.key("S-Left", "Shift+← · return to panel", spec.label)
                terminal.wait_screen(APP_HEADER_PATTERN, 45)
                terminal.remember("Returned to shared dashboard", "open-agent-view")
                sequence_wait(3.75)

            # The launch result selects the exact provider/session ID. Refuse
            # to record a rename unless that newly created row is actually
            # visible; otherwise a delayed provider-history flush could make
            # the demo rename the previously selected provider instead.
            row_label = SEQUENCE_ROW_LABELS[spec.id]
            terminal.wait_screen(
                rf"(?ms)^(?:Needs input|Working|Completed)[ \t]*$"
                rf"(?:\n[^\n]*){{0,24}}\n[^\n]*\b{re.escape(row_label)}\b",
                30,
            )
            terminal.wait_selected_row(row_label)

            renamed = f"{spec.id}-explanation"
            terminal.key("C-r", "Ctrl+R · rename session", "open-agent-view")
            terminal.wait_screen(r"rename session", 20)
            terminal.key("C-u", "Ctrl+U · clear name", "open-agent-view")
            sequence_type_text(
                terminal,
                renamed,
                f"Type · {renamed}",
                "open-agent-view",
                0.06,
            )
            sequence_wait(1.0)
            terminal.key("Enter", "Enter · save name", "open-agent-view")
            terminal.wait_screen(
                rf"(?m)^\s*[^\n]*\b{re.escape(renamed)}\b"
                rf"[^\n]*\b{re.escape(row_label)}\b",
                30,
            )
            terminal.remember("Renamed session visible", "open-agent-view")
            sequence_wait(1.125)

            sequence_type_line(
                terminal,
                "/harness",
                "Type /harness",
                "open-agent-view",
                0.07,
            )
            terminal.wait_screen(r"choose harness", 30)
            sequence_wait(1.5)
            end = terminal.timeline_time()
            previous_names.append(renamed)
            if write_stories:
                write_sequence_recording(
                    root,
                    output,
                    terminal,
                    spec,
                    start,
                    end,
                    previous_names[:-1],
                    action_start_index,
                )
            print(f"captured coherent {spec.label} story", flush=True)
            if capture_overview and spec.id == "kimi":
                target_spec = next(
                    candidate for candidate in SEQUENCE_DEMOS if candidate.id == "kimi"
                )
                # Record from the same private dashboard that created every
                # row. Restarting here would turn a managed-only overview into
                # provider-wide history and can also race supervisor shutdown.
                terminal.key("Escape", "Esc · close harness picker", "open-agent-view")
                terminal.wait_screen_without(r"choose harness", 30)
                terminal.key("Escape", "Esc · focus session list", "open-agent-view")
                terminal.wait_selected_row(target_spec.label, 20)
                capture_overview_story(
                    root,
                    output,
                    terminal,
                    previous_names,
                    target_spec,
                )
                sequence_type_line(
                    terminal,
                    "/harness",
                    "Type /harness",
                    "open-agent-view",
                    0.07,
                )
                terminal.wait_screen(r"choose harness", 30)
            if capture_rename and spec.id == "codex":
                capture_multi_session_rename(root, output, terminal, previous_names)
            if through == spec.id:
                break

        terminal.key("Escape", "Esc · close picker", "open-agent-view")
        terminal.key("Escape", "Esc · quit", "open-agent-view")
        terminal.finish()
    except Exception:
        for log_path in sorted((root / "state").glob("**/*.log")):
            with contextlib.suppress(OSError):
                detail = log_path.read_text(encoding="utf-8", errors="replace")[-2000:]
                for secret in RECORDER_SECRET_VALUES:
                    detail = detail.replace(secret, "[credential redacted]")
                print(f"provider log {log_path.relative_to(root)}:\n{detail}", file=sys.stderr)
        raise
    finally:
        if terminal is not None:
            terminal.finish()
        terminate_owned_processes(root)
        shutil.rmtree(root, ignore_errors=True)


def install_local_binary(repo: Path, root: Path) -> None:
    binary = repo / "target" / "release" / "open-agent-view"
    if not binary.is_file():
        raise RuntimeError("build target/release/open-agent-view before recording controls")
    bin_dir = root / "home" / ".local" / "bin"
    (bin_dir / "open-agent-view").symlink_to(binary.resolve())
    (bin_dir / "oav").symlink_to("open-agent-view")


def prepare_cursor_demo_wrapper(root: Path) -> None:
    """Use Cursor's documented trust flag without creating a prewarm chat.

    Cursor may show its workspace-trust overlay only after OAV has allocated
    and resumed the real chat. Accepting that overlay during the recording is
    racy because it can repaint over the first prompt. A separate native
    prewarm would itself create a provider conversation, so keep the real
    executable and add only Cursor's documented global ``--trust`` option to
    the invocations made by this disposable demo.
    """

    exposed = root / "home" / ".local" / "bin" / "cursor-agent"
    if not exposed.is_file():
        raise RuntimeError("real demo requires an exposed Cursor executable")
    executable = exposed.resolve(strict=True)
    exposed.unlink()
    write_private_text(
        exposed,
        "#!/bin/sh\n"
        f"exec {shlex.quote(str(executable))} --trust \"$@\"\n",
    )
    exposed.chmod(0o700)


def prewarm_sequence_harnesses(
    root: Path,
    environment: dict[str, str],
) -> None:
    """Warm real executable, catalog, and supervisor paths before recording.

    These probes run outside asciinema, use the same disposable HOME and
    credentials as the subsequent capture, and never create a conversation.
    That keeps first-process startup costs out of the story without inventing
    provider output or polluting the gradually accumulated session list.
    """

    work = root / "home" / "work" / "acme-dashboard"
    probes = [
        ["claude", "--help"],
        ["codex", "--version"],
        ["pi", "--offline", "--list-models"],
        ["opencode", "models"],
        ["cursor-agent", "models"],
        ["copilot", "--version"],
        ["agy", "models"],
        ["vibe", "--version"],
        ["muse", "--version"],
        ["qwen", "--version"],
        ["kimi", "provider", "list", "--json"],
        ["omp", "models", "list", "--no-extensions", "--json"],
        ["grok", "models"],
        ["kilo", "models", "openai"],
        ["openhands", "--version"],
        ["bash", "--version"],
        ["oav", "--json", "--cwd", str(work), "--history-limit", "1"],
    ]
    print("prewarming real harness executables and model catalogs…", flush=True)
    for command in probes:
        try:
            completed = subprocess.run(
                command,
                cwd=work,
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=60,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise RuntimeError(
                f"prewarm failed for {command[0]}: {error}"
            ) from error
        if completed.returncode != 0:
            raise RuntimeError(
                f"prewarm failed for {shlex.join(command)} with status "
                f"{completed.returncode}"
            )
    print("prewarmed all real harness paths", flush=True)


def start_control_dashboard(
    repo: Path,
    root: Path,
    demo: str,
    active_provider: str,
) -> RealTerminal:
    environment = base_environment(root)
    required_executables = {
        "all": ("claude",),
        "pi": ("pi",),
        "terminal": (),
    }[active_provider]
    expose_installed_providers(root, required_executables)
    if active_provider not in ("terminal", "all"):
        prepare_provider_login(active_provider, root, environment)
    install_local_binary(repo, root)
    work = root / "home" / "work" / "acme-dashboard"
    terminal = RealTerminal(demo, root, environment)
    selected_harness = "claude" if active_provider == "all" else active_provider
    disabled = [] if active_provider == "all" else provider_disable_flags(active_provider)
    command = [
        "oav",
        "--cwd",
        str(work),
        "--launch-cwd",
        str(work),
        "--refresh-ms",
        "30000",
        "--harness",
        selected_harness,
        *disabled,
    ]
    terminal.type_line(shlex.join(command), "Enter · launch oav", "Terminal", 0.001)
    terminal.wait_for(APP_HEADER_PATTERN, 45)
    time.sleep(0.8)
    return terminal


def prepare_control_fixture(root: Path) -> Path:
    """Create a dense, realistic multi-harness dashboard for control demos.

    The fixture feeds the released binary's normal discovery, grouping,
    selection, composer, and local-name code paths. It avoids inventing a
    terminal-looking web animation while keeping the rename story repeatable
    and free of provider API calls.
    """

    work = root / "home" / "work" / "acme-dashboard"
    now_ms = int(time.time() * 1000)
    rows = (
        ("ready_for_review", "claude", "release-review", "Release notes are ready for final review"),
        ("needs_input", "codex", "api-migration", "Choose the rollout window for the API migration"),
        ("needs_input", "pi", "test-triage", "Confirm which flaky tests should be quarantined"),
        ("needs_input", "opencode", "billing-cleanup", "Approval needed before updating billing fixtures"),
        ("working", "cursor", "frontend-polish", "Refining responsive layout and keyboard focus"),
        ("working", "github_copilot", "database-indexes", "Benchmarking the new query indexes"),
        ("working", "antigravity", "incident-report", "Tracing the production timeout regression"),
        ("working", "mistral_vibe", "cli-packaging", "Validating release archives on Linux and macOS"),
        ("completed", "muse_code", "docs-refresh", "Updated the operator guide and examples"),
        ("completed", "qwen_code", "auth-tests", "Authentication integration tests are passing"),
        ("completed", "kimi_code", "cache-profile", "Cache hit-rate analysis is complete"),
        ("completed", "claude", "accessibility", "Keyboard and screen-reader audit completed"),
        ("completed", "codex", "dependency-audit", "Dependency upgrade plan is ready"),
        ("completed", "pi", "metrics-cleanup", "Removed duplicate telemetry counters"),
        ("completed", "opencode", "release-notes", "Drafted release notes for the next version"),
        ("completed", "cursor", "mobile-layout", "Verified the compact dashboard layout"),
    )
    sessions = []
    for index, (state, provider, name, summary) in enumerate(rows):
        provider_id = provider.replace("_", "-")
        sessions.append(
            {
                "id": f"{provider_id}:host:control-{index:02d}",
                "provider_session_id": f"control-{index:02d}",
                "provider": provider,
                "runtime": {"kind": "host"},
                "kind": "managed",
                "name": name,
                "cwd": str(work),
                "state": state,
                "summary": summary,
                "raw_state": state,
                "pid": 4100 + index if state == "working" else None,
                "started_at": now_ms - (index + 2) * 240_000,
                "updated_at": now_ms - (index + 1) * 95_000,
                "pull_requests": None,
                "capabilities": ["inspect"],
            }
        )
    path = root / "state" / "control-sessions.json"
    write_private_text(
        path,
        json.dumps({"sessions": sessions, "warnings": []}, indent=2) + "\n",
    )
    return path


def start_fixture_control_dashboard(
    repo: Path,
    root: Path,
    demo: str,
) -> RealTerminal:
    environment = base_environment(root)
    install_local_binary(repo, root)
    fixture = prepare_control_fixture(root)
    work = root / "home" / "work" / "acme-dashboard"
    terminal = RealTerminal(demo, root, environment)
    command = [
        "oav",
        "--fixture",
        str(fixture),
        "--cwd",
        str(work),
        "--refresh-ms",
        "30000",
        "--history-limit",
        "40",
    ]
    terminal.type_line(shlex.join(command), "Enter · launch oav", "Terminal", 0.001)
    terminal.wait_for(APP_HEADER_PATTERN, 45)
    terminal.wait_screen(r"release-review.*Claude", 20)
    time.sleep(0.8)
    return terminal


def start_migration_control_dashboard(
    repo: Path,
    root: Path,
) -> RealTerminal:
    """Start a deterministic dashboard backed by a real native conversion.

    OAV reads a normal host-runtime fixture for the visible multi-session
    dashboard. The selected Claude ID also exists in the disposable HOME in
    Claude's native layout, so the installed session-migrate executable does
    the actual Claude-to-Codex conversion when the recording presses Ctrl+M.
    """

    environment = base_environment(root)
    install_local_binary(repo, root)
    bin_dir = root / "home" / ".local" / "bin"
    migrator = Path(require_program("session-migrate")).resolve(strict=True)
    (bin_dir / "session-migrate").symlink_to(migrator)

    fixture = prepare_control_fixture(root)
    document = json.loads(fixture.read_text(encoding="utf-8"))
    source_id = "30000000-0000-4000-8000-000000000000"
    source = document["sessions"][0]
    if source["provider"] != "claude" or source["name"] != "release-review":
        raise RuntimeError("migration demo fixture no longer starts on release-review")
    source["id"] = f"claude:host:{source_id}"
    source["provider_session_id"] = source_id
    write_private_text(fixture, json.dumps(document, indent=2) + "\n")

    work = root / "home" / "work" / "acme-dashboard"
    project_key = re.sub(r"[^A-Za-z0-9]", "-", str(work.resolve())) or "-"
    native_source = (
        root
        / "home"
        / ".claude"
        / "projects"
        / project_key
        / f"{source_id}.jsonl"
    )
    native_source.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    shutil.copyfile(repo / "fixtures" / "migration-demo-claude.jsonl", native_source)
    native_source.chmod(0o600)

    terminal = RealTerminal("migration", root, environment)
    command = [
        "oav",
        "--fixture",
        str(fixture),
        "--cwd",
        str(work),
        "--refresh-ms",
        "30000",
        "--history-limit",
        "40",
        "--no-host-providers",
        "--session-migrate-bin",
        "session-migrate",
    ]
    terminal.type_line(shlex.join(command), "Enter · launch oav", "Terminal", 0.001)
    terminal.wait_for(APP_HEADER_PATTERN, 45)
    terminal.wait_selected_row("release-review", 20)
    time.sleep(0.8)
    return terminal


def show_composer_guidance(
    terminal: RealTerminal,
    message: str,
    *,
    hold: float = 2.5,
) -> None:
    """Write a readable instruction in OAV's genuine bottom composer."""

    words = re.findall(r"[A-Za-z0-9+/]+", message)[:7]
    visible_pattern = r"\W+".join(re.escape(word) for word in words)
    terminal.type_text(message, "Read · dashboard guidance", "open-agent-view", 0.035)
    terminal.wait_screen(visible_pattern, 20)
    time.sleep(hold)
    terminal.key("C-u", "Ctrl+U · clear guidance", "open-agent-view")
    terminal.key("Escape", "Esc · return to dashboard", "open-agent-view")
    terminal.wait_screen_without(visible_pattern, 20)
    time.sleep(0.6)


def prepare_real_terminal_session(
    terminal: RealTerminal,
    name: str = "workspace shell",
    ready_message: str = "Managed terminal ready.",
) -> None:
    terminal.type_line(
        "/harness terminal",
        "Enter · select Terminal harness",
        "open-agent-view",
        0.04,
    )
    terminal.wait_screen(r"new tasks will use the Terminal harness", 20)
    terminal.type_line(
        name,
        "Enter · create terminal session",
        "open-agent-view",
        0.035,
    )
    terminal.wait_screen(r"[$#]\s*$", 30)
    terminal.type_line(
        f"printf '%s\\n' {shlex.quote(ready_message)}",
        "Enter · run command",
        "Terminal",
        0.012,
    )
    terminal.wait_screen(re.escape(ready_message), 15)
    terminal.key("S-Left", "Shift+← · return to oav", "Terminal")
    terminal.wait_screen(APP_HEADER_PATTERN, 30)
    terminal.wait_screen(re.escape(name), 20)
    time.sleep(0.8)


def capture_control(repo: Path, output: Path, demo: str) -> None:
    root = Path(tempfile.mkdtemp(prefix=f"oav-real-{demo}."))
    terminal: RealTerminal | None = None
    try:
        if demo == "migration":
            terminal = start_migration_control_dashboard(repo, root)
        elif demo == "rename":
            terminal = start_fixture_control_dashboard(repo, root, demo)
        else:
            active = "pi" if demo == "model" else ("all" if demo == "login" else "terminal")
            terminal = start_control_dashboard(repo, root, demo, active)

        if demo == "switch":
            for name, ready_message in (
                ("api-server", "API server shell ready."),
                ("test-watcher", "Test watcher shell ready."),
                ("release-shell", "Release shell ready."),
            ):
                prepare_real_terminal_session(terminal, name, ready_message)
        elif demo == "model":
            prepare_real_terminal_session(
                terminal,
                "workspace-shell",
                "Workspace shell stays available.",
            )
            terminal.type_line(
                "/harness pi",
                "Enter · select Pi harness",
                "open-agent-view",
                0.04,
            )
            terminal.wait_screen(r"new tasks will use the Pi harness", 20)
            time.sleep(0.8)
        elif demo == "login":
            prepare_real_terminal_session(
                terminal,
                "workspace-shell",
                "Workspace shell stays available during setup.",
            )

        start = terminal.repaint_start()
        action_start_index = len(terminal.actions)
        if demo == "migration":
            terminal.remember(
                "Selected · release-review in Claude",
                "open-agent-view",
            )
            time.sleep(1.6)
            terminal.literal_key(
                "\x1b[109;5u",
                "Ctrl+M · migrate selected session",
                "open-agent-view",
            )
            terminal.wait_screen(r"migrate session · target 1/14", 20)
            terminal.wait_screen(r"from\s+Claude", 20)
            time.sleep(1.4)
            terminal.key("Down", "↓ · preview Pi", "open-agent-view")
            terminal.wait_screen(r"migrate session · target 2/14", 20)
            time.sleep(0.8)
            terminal.key("Up", "↑ · choose Codex", "open-agent-view")
            terminal.wait_screen(r"migrate session · target 1/14", 20)
            time.sleep(1.0)
            terminal.key("Enter", "Enter · choose Codex", "open-agent-view")
            terminal.wait_screen(r"migrate to Codex · choose local name", 20)
            terminal.wait_screen(r"release-review \(Codex\)", 20)
            terminal.remember(
                "Default name · release-review (Codex)",
                "open-agent-view",
            )
            time.sleep(1.8)
            terminal.key("Enter", "Enter · migrate", "open-agent-view")
            terminal.wait_screen(r"migrated to Codex as release-review \(Codex\)", 30)
            terminal.wait_screen(r"Migrated from Claude", 20)
            terminal.remember(
                "Done · imported Codex session is visible",
                "open-agent-view",
            )
            time.sleep(3.0)
        elif demo == "rename":
            show_composer_guidance(
                terminal,
                "Select a session, then press Ctrl+R to give it a clear local name.",
                hold=2.8,
            )
            rename_steps = (
                ("release-review", "Claude", "launch-review"),
                ("api-migration", "Codex", "api-cutover"),
                ("test-triage", "Pi", "test-plan"),
            )
            for index, (old_name, provider, new_name) in enumerate(rename_steps):
                terminal.wait_selected_row(old_name, 20)
                time.sleep(0.8)
                terminal.key(
                    "C-r",
                    f"Ctrl+R · rename {provider} session",
                    "open-agent-view",
                )
                terminal.wait_screen(r"rename session", 20)
                terminal.wait_screen(re.escape(old_name), 20)
                time.sleep(1.0)
                terminal.key("C-u", "Ctrl+U · clear current name", "open-agent-view")
                terminal.type_text(
                    new_name,
                    f"Type · {new_name}",
                    "open-agent-view",
                    0.065,
                )
                time.sleep(0.8)
                terminal.key("Enter", "Enter · save local name", "open-agent-view")
                terminal.wait_screen(
                    rf"(?m)^\s*[^\n]*\b{re.escape(new_name)}\b"
                    rf"[^\n]*\b{re.escape(provider)}\b",
                    20,
                )
                terminal.remember(f"Saved · {new_name} remains {provider}", "open-agent-view")
                time.sleep(1.4)
                if index + 1 < len(rename_steps):
                    terminal.key("Down", "↓ · move to the next section", "open-agent-view")
                    if index == 0:
                        terminal.key("Down", "↓ · select the next agent", "open-agent-view")
                    terminal.wait_selected_row(rename_steps[index + 1][0], 20)

            show_composer_guidance(
                terminal,
                "Names stay on this dashboard; Claude, Codex, and Pi keep their own titles.",
                hold=3.0,
            )
            terminal.wait_screen(r"launch-review.*Claude", 20)
            terminal.wait_screen(r"api-cutover.*Codex", 20)
            terminal.wait_screen(r"test-plan.*Pi", 20)
            terminal.remember("Three renamed sessions visible", "open-agent-view")
            time.sleep(2.5)
        elif demo == "switch":
            show_composer_guidance(
                terminal,
                "Use Up and Down to choose a session. Press Right or Enter to open it.",
                hold=2.8,
            )
            terminal.wait_selected_row("release-shell", 20)
            terminal.key("Down", "↓ · select test-watcher", "open-agent-view")
            terminal.wait_selected_row("test-watcher", 20)
            time.sleep(1.0)
            terminal.key("Right", "→ · open selected session", "open-agent-view")
            terminal.wait_native_screen(r"Test watcher shell ready\.", 20)
            time.sleep(1.5)
            terminal.key("Left", "← · arm return at empty prompt", "Terminal")
            terminal.wait_screen(r"Press ← again", 10)
            time.sleep(1.5)
            terminal.key("Left", "← again · return to dashboard", "Terminal")
            terminal.wait_screen(APP_HEADER_PATTERN, 20)
            show_composer_guidance(
                terminal,
                "At an empty prompt, Left twice returns. Shift+Left returns immediately.",
                hold=3.0,
            )
            terminal.key("Enter", "Enter · reopen test-watcher", "open-agent-view")
            terminal.wait_native_screen(r"Test watcher shell ready\.", 20)
            time.sleep(1.2)
            terminal.key("S-Left", "Shift+← · return immediately", "Terminal")
            terminal.wait_screen(APP_HEADER_PATTERN, 20)
            show_composer_guidance(
                terminal,
                "The three sessions keep running while the shared dashboard is in front.",
                hold=3.0,
            )
            terminal.remember("All managed sessions remain visible", "open-agent-view")
            time.sleep(2.0)
        elif demo == "model":
            show_composer_guidance(
                terminal,
                "/model chooses the model for the next Pi session.",
                hold=2.8,
            )
            terminal.type_line("/model", "Type /model", "open-agent-view", 0.07)
            terminal.wait_screen(r"choose Pi model", 45)
            terminal.wait_screen(r"results", 45)
            time.sleep(2.0)
            terminal.key("Down", "↓ · browse available models", "open-agent-view")
            time.sleep(0.8)
            terminal.key("Up", "↑ · return to first result", "open-agent-view")
            time.sleep(0.8)
            terminal.type_text(
                "gpt-5.4",
                "Type · search gpt-5.4",
                "open-agent-view",
                0.09,
            )
            terminal.wait_screen(r"gpt-5\.4", 30)
            time.sleep(2.0)
            terminal.key("Enter", "Enter · select the filtered model", "open-agent-view")
            terminal.wait_screen(r"model\s+[^\n]*gpt-5\.4", 20)
            show_composer_guidance(
                terminal,
                "The composer now shows the exact model the next Pi task will use.",
                hold=3.2,
            )
            terminal.remember("Selected Pi model remains visible", "open-agent-view")
            time.sleep(2.0)
        elif demo == "login":
            show_composer_guidance(
                terminal,
                "/setup claude checks installation and opens Claude's native sign-in.",
                hold=3.0,
            )
            terminal.type_line("/setup claude", "Type /setup claude", "open-agent-view", 0.06)
            terminal.wait_screen(r"interactive login now\?", 45)
            terminal.remember("Setup check complete", "Harness setup")
            time.sleep(3.0)
            terminal.key("Enter", "Enter · open native login", "Harness setup")
            terminal.wait_screen(r"Opening Claude Code login|browser|sign[ -]?in|log[ -]?in", 60)
            terminal.remember("Native login ready", "Claude Code login")
            time.sleep(4.0)
            terminal.key("S-Left", "Shift+← · background setup", "Claude Code login")
            terminal.wait_screen(APP_HEADER_PATTERN, 30)
            show_composer_guidance(
                terminal,
                "Setup stays managed in the dashboard while you continue other work.",
                hold=3.2,
            )
            terminal.remember("Returned without losing setup", "open-agent-view")
            time.sleep(2.2)
        else:
            raise RuntimeError(f"unknown control recording: {demo}")

        time.sleep(1.2)
        end = terminal.timeline_time()
        terminal.key("Escape", "Esc · close", "open-agent-view")
        terminal.finish()
        target = output / f"{demo}.cast"
        write_trimmed_cast(
            terminal.raw_cast,
            target,
            start,
            end,
            public_path_replacements(root),
        )
        primed_lead = prime_first_terminal_frame(target)
        actions = [
            {
                **item,
                "at": max(
                    0.0,
                    round(float(item["at"]) - start - primed_lead, 3),
                ),
            }
            for item in terminal.actions[action_start_index:]
            if start <= float(item["at"]) <= end
        ]
        action_path = output / f"{demo}.actions.json"
        action_path.write_text(
            json.dumps(
                {
                    "duration": end - start + 1.35 - primed_lead,
                    "proof": "real-open-agent-view-tui",
                    "sequence": {
                        "rename": "guide-rename-three-multi-harness-sessions",
                        "switch": "guide-select-open-double-left-reopen-shift-left",
                        "model": "guide-browse-search-select-pi-model",
                        "login": "guide-check-open-native-login-background",
                        "migration": (
                            "guide-ctrl-m-choose-target-confirm-default-name"
                        ),
                    }[demo],
                    "actions": actions,
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        action_path.chmod(0o644)
        required = {
            "rename": [
                "release-review",
                "database-indexes",
                "launch-review",
                "api-cutover",
                "test-plan",
            ],
            "switch": [
                "release-shell",
                "test-watcher",
                "api-server",
                "Press ← again",
            ],
            "model": [
                "workspace-shell",
                "choose Pi model",
                "gpt-5.4",
            ],
            "login": [
                "interactive login now?",
                "Opening Claude Code login",
            ],
            "migration": [
                "release-review",
                "migrate session",
                "release-review (Codex)",
                "Migrated from Claude",
            ],
        }[demo]
        validate_public_cast(target, [APP_HEADER, *required])
        print(f"captured real Open Agent View {demo} controls: {target}")
    finally:
        if terminal is not None:
            terminal.finish()
        terminate_owned_processes(root)
        shutil.rmtree(root, ignore_errors=True)


def capture_claude(repo: Path, output: Path) -> None:
    root = Path(tempfile.mkdtemp(prefix="oav-real-claude."))
    terminal: RealTerminal | None = None
    try:
        environment = base_environment(root)
        expose_installed_providers(root, ("claude",))
        binary = repo / "target" / "release" / "open-agent-view"
        if not binary.is_file():
            raise RuntimeError("build target/release/open-agent-view before recording Claude")
        bin_dir = root / "home" / ".local" / "bin"
        (bin_dir / "open-agent-view").symlink_to(binary.resolve())
        (bin_dir / "oav").symlink_to("open-agent-view")

        claude_config = root / "claude-config"
        private_copy(Path.home() / ".claude" / ".credentials.json", claude_config / ".credentials.json")
        private_copy(Path.home() / ".claude.json", claude_config / ".claude.json")
        environment.update(
            {
                "CLAUDE_CONFIG_DIR": str(claude_config),
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
                "DISABLE_AUTOUPDATER": "1",
            }
        )

        work = root / "home" / "work" / "acme-dashboard"
        terminal = RealTerminal("claude", root, environment)
        command = " ".join(
            [
                "oav",
                "--cwd",
                shlex.quote(str(work)),
                "--launch-cwd",
                shlex.quote(str(work)),
                "--refresh-ms",
                "30000",
                "--no-host-codex",
                "--no-host-pi",
                "--no-host-opencode",
                "--no-host-cursor",
                "--no-host-copilot",
                "--no-host-antigravity",
            ]
        )
        terminal.type_line(command, "Enter · launch oav", "Terminal", 0.001)
        terminal.wait_for(APP_HEADER_PATTERN, 45)
        time.sleep(0.8)
        start = terminal.repaint_start()
        terminal.type_line("/harness", "Type /harness", "open-agent-view", 0.07)
        terminal.wait_for(r"choose harness", 20)
        terminal.key("Enter", "Enter · choose Claude", "open-agent-view")
        time.sleep(0.45)

        first_prompt = "Reply exactly: One view for every coding agent."
        terminal.type_line(first_prompt, "Enter · start Claude", "open-agent-view", 0.018)
        terminal.wait_for(r"Claude Code v", 55)
        terminal.wait_screen_occurrences("One view for every coding agent.")
        time.sleep(1.0)

        follow_up = "Now reply exactly: Return without losing the session."
        terminal.type_line(follow_up, "Enter · send follow-up", "Claude Code", 0.016)
        terminal.wait_screen_occurrences("Return without losing the session.")
        terminal.remember("Response ready", "Claude Code")
        time.sleep(2.2)

        terminal.key("S-Left", "Shift+← · return to oav", "Claude Code")
        terminal.wait_screen(APP_HEADER_PATTERN, 20)
        time.sleep(1.6)
        terminal.key("Right", "→ · reopen Claude", "open-agent-view")
        terminal.wait_screen(r"Claude Code v", 45)
        terminal.remember("Session reopened", "Claude Code")
        time.sleep(2.2)
        terminal.key("S-Left", "Shift+← · return to oav", "Claude Code")
        terminal.wait_screen(APP_HEADER_PATTERN, 20)
        time.sleep(1.2)
        end = terminal.timeline_time()

        terminal.key("Escape", "Esc · quit", "open-agent-view")
        terminal.finish()
        target = output / "claude.cast"
        write_trimmed_cast(
            terminal.raw_cast,
            target,
            start,
            end,
            public_path_replacements(root),
        )
        actions = [
            {**item, "at": max(0.0, round(float(item["at"]) - start, 3))}
            for item in terminal.actions
            if start <= float(item["at"]) <= end
        ]
        action_path = output / "claude.actions.json"
        action_path.write_text(
            json.dumps({"duration": end - start + 1.35, "actions": actions}, indent=2) + "\n",
            encoding="utf-8",
        )
        action_path.chmod(0o644)
        compact_recording(target, action_path)
        validate_public_cast(
            target,
            [
                APP_HEADER,
                "choose harness",
                "Claude Code v",
                "One view for every coding agent",
                "Return without losing the session",
            ],
        )
        print(f"captured real Open Agent View → Claude TUI: {target}")
    finally:
        if terminal is not None:
            terminal.finish()
        terminate_owned_processes(root)
        shutil.rmtree(root, ignore_errors=True)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "demo", choices=("setup", "sequence", "overview", "claude", *PROVIDER_DEMOS, *CONTROL_DEMOS)
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=None,
        help="destination (default: website/public/demos)",
    )
    parser.add_argument(
        "--through",
        choices=tuple(spec.id for spec in SEQUENCE_DEMOS),
        default=None,
        help="for sequence, stop after this harness (useful for recorder validation)",
    )
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    output = args.output_dir or repo / "website" / "public" / "demos"
    for program in ("asciinema", "curl", "tmux"):
        require_program(program)
    if args.demo == "setup":
        capture_setup(repo, output)
    elif args.demo == "sequence":
        capture_provider_sequence(repo, output, through=args.through)
    elif args.demo == "overview":
        capture_provider_sequence(
            repo,
            output,
            through="kimi",
            capture_overview=True,
            write_stories=False,
        )
    elif args.demo == "claude":
        capture_claude(repo, output)
    elif args.demo in PROVIDER_DEMOS:
        capture_provider(repo, output, PROVIDER_DEMOS[args.demo])
    else:
        capture_control(repo, output, args.demo)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, subprocess.CalledProcessError) as error:
        print(f"real demo capture failed: {error}", file=sys.stderr)
        raise SystemExit(1)
