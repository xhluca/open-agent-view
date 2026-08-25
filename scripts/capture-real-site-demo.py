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

from compact_real_recordings import compact_recording


COLS = 132
ROWS = 34
EXPECTED_VERSION = "0.1.35"
INSTALL_COMMAND = "curl -fsSL https://open-agent-view.github.io/install.sh | bash"
SECRET_PATTERN = re.compile(
    r"api[_-]?key|oauth[_-]?token|authorization:\s*bearer|ghp_|sk-[A-Za-z0-9]",
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
    kept.append([round(float(kept[-1][0]) + 1.35, 6), "o", "\x1b[0m"])
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(
        "\n".join(json.dumps(record, ensure_ascii=False) for record in (header, *kept))
        + "\n",
        encoding="utf-8",
    )
    target.chmod(0o644)


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
        exports = " ".join(
            f"{key}={shlex.quote(value)}" for key, value in sorted(environment.items())
        )
        inner_shell = f"env {exports} bash --noprofile --norc"
        command = (
            f"{shlex.quote(require_program('asciinema'))} rec "
            f"--overwrite --quiet --idle-time-limit 0.65 "
            f"--command {shlex.quote(inner_shell)} "
            f"{shlex.quote(str(self.raw_cast))}"
        )
        run(["tmux", "send-keys", "-t", self.session, "-l", command])
        run(["tmux", "send-keys", "-t", self.session, "Enter"])
        self.wait_for_recorded("OAV-DEMO-READY", 20)

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
                "at": round(cast_time(self.raw_cast), 3),
                "action": label,
                "window": window,
            }
        )

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
    (root / "home" / "work" / "acme-dashboard" / "AGENTS.md").write_text(
        "This is a disposable Open Agent View demo workspace.\n",
        encoding="utf-8",
    )
    values = {key: str(value) for key, value in directories.items()}
    values.update(
        {
            "PATH": f"{bin_dir}:{os.environ.get('PATH', '/usr/bin:/bin')}",
            # The OSC marker is invisible in the terminal, but lets the driver
            # prove that asciinema's inner shell—not tmux's outer shell—is ready.
            "PS1": "$ \\[\\e]777;OAV-DEMO-READY\\a\\]",
            "TERM": "xterm-256color",
            "NO_COLOR": "0",
            "OAV_INSTALL_DIR": str(bin_dir),
            # The application repository is currently private.  The public
            # installer uses gh for its authenticated release fallback; this
            # points at the existing config without copying it into the demo.
            "GH_CONFIG_DIR": os.environ.get(
                "GH_CONFIG_DIR", str(Path.home() / ".config" / "gh")
            ),
        }
    )
    for key in ("LANG", "LC_ALL", "LC_CTYPE", "SSL_CERT_FILE", "SSL_CERT_DIR"):
        if value := os.environ.get(key):
            values[key] = value
    return values


def expose_installed_providers(root: Path) -> None:
    """Expose the real installed CLIs inside the disposable demo home."""

    candidates = {
        "claude": [Path.home() / ".local/bin/claude"],
        "codex": [Path.home() / ".npm-global/bin/codex", Path.home() / ".local/bin/codex"],
        "pi": [Path.home() / ".local/bin/pi", Path.home() / ".npm-global/bin/pi"],
        "opencode": [Path.home() / ".opencode/bin/opencode", Path.home() / ".local/bin/opencode"],
        "cursor-agent": [Path.home() / ".local/bin/cursor-agent"],
        "copilot": [Path.home() / ".npm-global/bin/copilot", Path.home() / ".local/bin/copilot"],
        "agy": [Path.home() / ".local/bin/agy"],
    }
    destination = root / "home" / ".local" / "bin"
    for name, paths in candidates.items():
        source = next((path for path in paths if path.is_file() and os.access(path, os.X_OK)), None)
        if source is None:
            raise RuntimeError(f"real demo requires installed provider executable: {name}")
        (destination / name).symlink_to(source.resolve())


def validate_public_cast(path: Path, required: list[str]) -> None:
    visible = visible_cast(path)
    compact = re.sub(r"\s+", "", visible)
    for value in required:
        if value not in visible and re.sub(r"\s+", "", value) not in compact:
            raise RuntimeError(f"real cast {path.name} is missing {value!r}")
    raw = path.read_text(encoding="utf-8")
    if SECRET_PATTERN.search(raw) or EMAIL_PATTERN.search(raw):
        raise RuntimeError(f"refusing to publish credential-like text in {path.name}")
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
        expose_installed_providers(root)
        terminal = RealTerminal("setup", root, environment)
        terminal.type_line(INSTALL_COMMAND, "Enter · install", "Terminal", 0.012)
        terminal.wait_for(r"installed shorthand:\s*opav", 120)
        time.sleep(0.6)
        terminal.type_line("opav", "Enter · launch opav", "Terminal", 0.08)
        terminal.wait_for(r"Open Agent View v0\.1\.35", 45)
        time.sleep(1.0)
        terminal.type_line("/harness", "Type /harness", "open-agent-view", 0.08)
        terminal.wait_for(r"choose harness", 20)
        terminal.key("Down", "↓ · highlight Codex", "open-agent-view")
        terminal.key("Up", "↑ · highlight Claude", "open-agent-view")
        time.sleep(1.0)
        end = cast_time(terminal.raw_cast)
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
            [INSTALL_COMMAND, "Open Agent View v0.1.35", "choose harness", "GitHub Copilot", "Terminal"],
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


@dataclass(frozen=True)
class ProviderDemo:
    id: str
    label: str
    cli_value: str
    ready_pattern: str
    model: str | None = None


PROVIDER_DEMOS = {
    "codex": ProviderDemo("codex", "OpenAI Codex", "codex", r"Codex|OpenAI"),
    "pi": ProviderDemo("pi", "Pi", "pi", r"Pi|pi"),
    "opencode": ProviderDemo(
        "opencode", "OpenCode", "opencode", r"OpenCode|opencode", "openai/gpt-5.6-luna"
    ),
    "cursor": ProviderDemo("cursor", "Cursor", "cursor", r"Cursor|cursor", "auto"),
    "copilot": ProviderDemo("copilot", "GitHub Copilot", "copilot", r"Copilot|copilot"),
    "antigravity": ProviderDemo(
        "antigravity",
        "Antigravity",
        "antigravity",
        r"Antigravity|antigravity",
        "gemini-3.1-pro-high",
    ),
    "terminal": ProviderDemo("terminal", "Terminal", "terminal", r"[$#]\s*$"),
}

CONTROL_DEMOS = ("rename", "switch", "model", "login")


def provider_disable_flags(active: str) -> list[str]:
    flags = []
    for provider in ("claude", "codex", "pi", "opencode", "cursor", "copilot", "antigravity"):
        if provider != active:
            flags.append(f"--no-host-{provider}")
    return flags


def capture_provider(repo: Path, output: Path, spec: ProviderDemo) -> None:
    root = Path(tempfile.mkdtemp(prefix=f"oav-real-{spec.id}."))
    terminal: RealTerminal | None = None
    try:
        environment = base_environment(root)
        expose_installed_providers(root)
        prepare_provider_login(spec.id, root, environment)
        binary = repo / "target" / "release" / "open-agent-view"
        if not binary.is_file():
            raise RuntimeError("build target/release/open-agent-view before recording providers")
        bin_dir = root / "home" / ".local" / "bin"
        (bin_dir / "open-agent-view").symlink_to(binary.resolve())
        (bin_dir / "opav").symlink_to("open-agent-view")

        work = root / "home" / "work" / "acme-dashboard"
        terminal = RealTerminal(spec.id, root, environment)
        command = [
            "opav",
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
            shlex.join(command), "Enter · launch opav", "Terminal", 0.001
        )
        terminal.wait_for(r"Open Agent View v0\.1\.35", 45)
        time.sleep(0.8)
        start = terminal.repaint_start()
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

        time.sleep(0.8)
        terminal.key("S-Left", "Shift+← · return to opav", spec.label)
        terminal.wait_screen(r"Open Agent View v0\.1\.35", 30)
        time.sleep(0.8)
        terminal.key("Right", f"→ · reopen {spec.label}", "open-agent-view")
        terminal.wait_screen(spec.ready_pattern, 60)
        time.sleep(0.8)
        terminal.key("S-Left", "Shift+← · return to opav", spec.label)
        terminal.wait_screen(r"Open Agent View v0\.1\.35", 30)
        time.sleep(0.8)
        end = cast_time(terminal.raw_cast)

        terminal.key("Escape", "Esc · quit", "open-agent-view")
        terminal.finish()
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
            json.dumps({"duration": end - start + 1.35, "actions": actions}, indent=2) + "\n",
            encoding="utf-8",
        )
        action_path.chmod(0o644)
        compact_recording(target, action_path)
        required = ["Open Agent View v0.1.35", "choose harness", spec.label]
        required.extend(["One dashboard, every harness.", "Session still here."])
        validate_public_cast(target, required)
        print(f"captured real Open Agent View → {spec.label} TUI: {target}")
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
    (bin_dir / "opav").symlink_to("open-agent-view")


def start_control_dashboard(
    repo: Path,
    root: Path,
    demo: str,
    active_provider: str,
) -> RealTerminal:
    environment = base_environment(root)
    expose_installed_providers(root)
    if active_provider not in ("terminal", "all"):
        prepare_provider_login(active_provider, root, environment)
    install_local_binary(repo, root)
    work = root / "home" / "work" / "acme-dashboard"
    terminal = RealTerminal(demo, root, environment)
    selected_harness = "claude" if active_provider == "all" else active_provider
    disabled = [] if active_provider == "all" else provider_disable_flags(active_provider)
    command = [
        "opav",
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
    terminal.type_line(shlex.join(command), "Enter · launch opav", "Terminal", 0.001)
    terminal.wait_for(r"Open Agent View v0\.1\.35", 45)
    time.sleep(0.8)
    return terminal


def prepare_real_terminal_session(terminal: RealTerminal) -> None:
    terminal.type_line("/harness", "Type /harness", "open-agent-view", 0.06)
    terminal.wait_for(r"choose harness", 20)
    terminal.key("Enter", "Enter · choose Terminal", "open-agent-view")
    terminal.type_line(
        "workspace shell",
        "Enter · create terminal session",
        "open-agent-view",
        0.035,
    )
    terminal.wait_screen(r"[$#]\s*$", 30)
    terminal.type_line(
        "printf 'Managed terminal ready.\\n'",
        "Enter · run command",
        "Terminal",
        0.012,
    )
    terminal.wait_screen(r"Managed terminal ready\.", 15)
    terminal.key("S-Left", "Shift+← · return to opav", "Terminal")
    terminal.wait_screen(r"Open Agent View v0\.1\.35", 30)
    terminal.wait_screen(r"workspace shell", 20)
    time.sleep(0.8)


def capture_control(repo: Path, output: Path, demo: str) -> None:
    root = Path(tempfile.mkdtemp(prefix=f"oav-real-{demo}."))
    terminal: RealTerminal | None = None
    try:
        active = "pi" if demo == "model" else ("all" if demo == "login" else "terminal")
        terminal = start_control_dashboard(repo, root, demo, active)

        if demo in ("rename", "switch"):
            prepare_real_terminal_session(terminal)

        start = terminal.repaint_start()
        if demo == "rename":
            terminal.key("C-r", "Ctrl+R · rename session", "open-agent-view")
            terminal.wait_screen(r"rename session", 15)
            terminal.key("C-u", "Ctrl+U · clear current name", "open-agent-view")
            terminal.type_text(
                "release workspace",
                "Type release workspace",
                "open-agent-view",
                0.065,
            )
            terminal.key("Enter", "Enter · save name", "open-agent-view")
            terminal.wait_screen(r"release workspace", 15)
        elif demo == "switch":
            terminal.key("Right", "→ · enter selected session", "open-agent-view")
            terminal.wait_screen(r"Managed terminal ready\.", 20)
            terminal.key("Left", "← · arm return", "Terminal")
            terminal.wait_screen(r"Press ← again", 10)
            terminal.key("Left", "← · return to opav", "Terminal")
            terminal.wait_screen(r"Open Agent View v0\.1\.35", 20)
            terminal.key("Right", "→ · reopen session", "open-agent-view")
            terminal.wait_screen(r"Managed terminal ready\.", 20)
            terminal.key("S-Left", "Shift+← · return immediately", "Terminal")
            terminal.wait_screen(r"Open Agent View v0\.1\.35", 20)
        elif demo == "model":
            terminal.type_line("/model", "Type /model", "open-agent-view", 0.07)
            terminal.wait_screen(r"choose Pi model", 45)
            terminal.wait_screen(r"results", 45)
            terminal.key("Down", "↓ · next model", "open-agent-view")
            terminal.key("Down", "↓ · next model", "open-agent-view")
            terminal.key("Enter", "Enter · select exact model", "open-agent-view")
            terminal.wait_screen(r"model", 15)
        elif demo == "login":
            terminal.type_line("/setup", "Type /setup", "open-agent-view", 0.07)
            terminal.wait_screen(r"interactive login now\?", 45)
        else:
            raise RuntimeError(f"unknown control recording: {demo}")

        time.sleep(1.2)
        end = cast_time(terminal.raw_cast)
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
        actions = [
            {**item, "at": max(0.0, round(float(item["at"]) - start, 3))}
            for item in terminal.actions
            if start <= float(item["at"]) <= end
        ]
        action_path = output / f"{demo}.actions.json"
        action_path.write_text(
            json.dumps({"duration": end - start + 1.35, "actions": actions}, indent=2)
            + "\n",
            encoding="utf-8",
        )
        action_path.chmod(0o644)
        required = {
            "rename": ["rename session", "release workspace"],
            "switch": ["Managed terminal ready.", "Press ← again"],
            "model": ["choose Pi model"],
            "login": ["interactive login now?"],
        }[demo]
        validate_public_cast(target, ["Open Agent View v0.1.35", *required])
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
        expose_installed_providers(root)
        binary = repo / "target" / "release" / "open-agent-view"
        if not binary.is_file():
            raise RuntimeError("build target/release/open-agent-view before recording Claude")
        bin_dir = root / "home" / ".local" / "bin"
        (bin_dir / "open-agent-view").symlink_to(binary.resolve())
        (bin_dir / "opav").symlink_to("open-agent-view")

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
                "opav",
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
        terminal.type_line(command, "Enter · launch opav", "Terminal", 0.001)
        terminal.wait_for(r"Open Agent View v0\.1\.35", 45)
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
        time.sleep(1.0)

        terminal.key("S-Left", "Shift+← · return to opav", "Claude Code")
        terminal.wait_screen(r"Open Agent View v0\.1\.35", 20)
        time.sleep(0.8)
        terminal.key("Right", "→ · reopen Claude", "open-agent-view")
        terminal.wait_screen(r"Claude Code v", 45)
        time.sleep(1.0)
        terminal.key("S-Left", "Shift+← · return to opav", "Claude Code")
        terminal.wait_screen(r"Open Agent View v0\.1\.35", 20)
        time.sleep(0.8)
        end = cast_time(terminal.raw_cast)

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
                "Open Agent View v0.1.35",
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
        "demo", choices=("setup", "claude", *PROVIDER_DEMOS, *CONTROL_DEMOS)
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=None,
        help="destination (default: website/public/demos)",
    )
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    output = args.output_dir or repo / "website" / "public" / "demos"
    for program in ("asciinema", "curl", "tmux"):
        require_program(program)
    if args.demo == "setup":
        capture_setup(repo, output)
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
