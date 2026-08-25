async function copyText(text) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  const input = document.createElement("textarea");
  input.value = text;
  input.setAttribute("readonly", "");
  input.style.position = "fixed";
  input.style.opacity = "0";
  document.body.append(input);
  input.select();
  const copied = document.execCommand("copy");
  input.remove();
  if (!copied) throw new Error("clipboard copy was refused");
}

function enableCopyCommands() {
  for (const button of document.querySelectorAll("[data-copy-command]")) {
    button.dataset.copyReady = "true";
  }
}

window.addEventListener("load", () => window.setTimeout(enableCopyCommands, 750), { once: true });

document.addEventListener("click", async (event) => {
  const button = event.target.closest?.("[data-copy-command][data-copy-ready=true]");
  if (!button) return;
  const label = button.querySelector("b");
  try {
    await copyText(button.dataset.copyCommand);
    label.textContent = "Copied";
  } catch {
    label.textContent = "Select";
  }
  window.setTimeout(() => { label.textContent = "Copy"; }, 1800);
});

const providers = {
  claude: {
    label: "Claude Code",
    title: "accessibility-audit",
    task: "Audit the onboarding flow for keyboard traps",
    model: "sonnet",
    first: "I found two focus traps and one missing accessible name.",
    follow: "Fix them and run the focused browser checks.",
    final: "Fixed all three. The keyboard and accessibility checks pass.",
  },
  codex: {
    label: "OpenAI Codex",
    title: "database-migration",
    task: "Add a reversible migration for the sessions index",
    model: "gpt-5",
    first: "The migration and rollback path are implemented.",
    follow: "Add a regression test for an interrupted rollback.",
    final: "Added it. The migration suite passes with the new failure case.",
  },
  pi: {
    label: "Pi",
    title: "test-coverage",
    task: "Find the untested session lifecycle branches",
    model: "provider/default",
    first: "Three ownership transitions lacked direct coverage.",
    follow: "Cover those transitions without broad fixtures.",
    final: "Added focused tests for all three transitions. They pass.",
  },
  opencode: {
    label: "OpenCode",
    title: "api-cleanup",
    task: "Simplify the provider discovery interface",
    model: "provider/model",
    first: "I can remove two adapters and preserve the public contract.",
    follow: "Make that change and document the compatibility boundary.",
    final: "The smaller interface is implemented and documented.",
  },
  cursor: {
    label: "Cursor",
    title: "responsive-layout",
    task: "Fix the mobile overflow in the session table",
    model: "auto",
    first: "The provider column forces the table past 390 pixels.",
    follow: "Keep provider identity visible while fixing the overflow.",
    final: "Provider marks now collapse cleanly and the phone test passes.",
  },
  copilot: {
    label: "GitHub Copilot",
    title: "release-notes",
    task: "Draft release notes from the verified changes",
    model: "default",
    first: "I grouped the changes by workflow, safety, and performance.",
    follow: "Lead with the user-visible improvements.",
    final: "Reordered and tightened. The notes now open with outcomes.",
  },
  antigravity: {
    label: "Antigravity",
    title: "architecture-map",
    task: "Explain the ownership boundary with a clean diagram",
    model: "gemini/default",
    first: "The clearest story is observe, index, then explicit action.",
    follow: "Show where the native harness remains authoritative.",
    final: "Added the native boundary and the exact authority check.",
  },
  terminal: {
    label: "Terminal",
    title: "verification-shell",
    task: "Run the focused checks and keep the shell available",
    model: "shell",
    first: "npm test\n✓ 4 rendered tests passed\n✓ 3 browser tests passed",
    follow: "git status --short",
    final: "(clean)",
  },
};

const providerOrder = Object.keys(providers);
const cyan = (value) => `<span class="term-cyan">${value}</span>`;
const green = (value) => `<span class="term-green">${value}</span>`;
const amber = (value) => `<span class="term-amber">${value}</span>`;
const dim = (value) => `<span class="term-dim">${value}</span>`;
const strong = (value) => `<span class="term-strong">${value}</span>`;
const selected = (value) => `<span class="term-selected">${value}</span>`;
const prompt = (value) => `<span class="term-prompt">${value}</span>`;

function shell(lines) {
  return lines.join("\n");
}

function storyFrame(at, screen, window = "open-agent-view", action = "Waiting") {
  return { at, screen, window, action };
}

function dashboard(ids, active = "", composer = "describe a task · /help for commands", notice = "") {
  const rows = ids.map((id) => {
    const provider = providers[id];
    const symbol = id === active ? amber("✱") : green("•");
    const row = ` ${symbol} ${strong(provider.title.padEnd(23))} ${cyan(provider.label.padEnd(16))} ${dim(provider.final)}`;
    return id === active ? selected(row) : row;
  }).join("\n");
  const working = ids.length ? ids.length : 0;
  return shell([
    `  ${cyan("◇◇")}  ${strong("Open Agent View v0.1.35")}`,
    ` ${cyan("◇  ◇")} ${providerOrder.slice(0, Math.max(ids.length, 1)).map((id) => providers[id].label).join(" + ")} · ${dim("/work/acme-dashboard")}`,
    `  ${cyan("◇◇")} 0 awaiting input · ${working} working · 0 completed · status view`,
    "",
    strong("Working"),
    rows || ` ${dim("Launch the first task below. Every demo stays in this workspace.")}`,
    "",
    dim("────────────────────────────────────────────────────────────────────────────────────────────────────────────"),
    `${prompt("❯")} ${composer}`,
    dim("────────────────────────────────────────────────────────────────────────────────────────────────────────────"),
    notice ? amber(notice) : dim("tab harness · shift+tab model · enter create · ? shortcuts"),
  ]);
}

function nativeConversation(id, phase) {
  const provider = providers[id];
  const lines = [
    `${cyan(provider.label)} · ${dim("/work/acme-dashboard")} · model ${provider.model}`,
    "",
    `${strong("You")}  ${provider.task}`,
  ];
  if (phase >= 1) lines.push("", `${cyan("Agent")}  ${provider.first}`);
  if (phase >= 2) lines.push("", `${strong("You")}  ${provider.follow}`);
  if (phase >= 3) lines.push("", `${cyan("Agent")}  ${provider.final}`);
  lines.push("", dim("← at an empty boundary: press again to return · Shift+←: return now"));
  return shell(lines);
}

function picker(selectedId = "claude") {
  const rows = providerOrder.map((id, index) => {
    const provider = providers[id];
    const text = `${String(index + 1).padStart(2, "0")}  ${provider.label.padEnd(18)} ${dim(id === "terminal" ? "local shell" : "coding harness")}`;
    return id === selectedId ? selected(`› ${text}`) : `  ${text}`;
  });
  return shell([
    `  ${cyan("◇◇")}  ${strong("Open Agent View · new task")}`,
    `  ${dim("/work/acme-dashboard")}`,
    "",
    strong("Choose harness"),
    ...rows,
    "",
    dim("↑/↓ move · enter select · esc back"),
  ]);
}

function overviewStory() {
  const frames = [
    storyFrame(0, shell([`${prompt("$")} opav`, dim("Opening Open Agent View…")]), "Terminal", "Enter · launch opav"),
    storyFrame(1.8, dashboard([]), "open-agent-view", "Dashboard opened"),
    storyFrame(4, dashboard([], "", `${strong("Audit the onboarding flow for keyboard traps")}  ${dim("harness Claude Code")}`), "open-agent-view", "Typed a new task"),
    storyFrame(6, nativeConversation("claude", 1), "Claude Code", "Enter · launch session"),
    storyFrame(8.5, nativeConversation("claude", 3), "Claude Code", "Enter · send follow-up"),
    storyFrame(11, dashboard(["claude"], "claude", "describe a task · /help for commands", "Returned from Claude Code"), "open-agent-view", "Shift+← · return"),
  ];
  providerOrder.slice(1).forEach((id, index) => {
    const visible = providerOrder.slice(0, index + 2);
    frames.push(storyFrame(
      13 + index * 2.1,
      dashboard(visible, id, providers[id].task, `Launched ${providers[id].label} in /work/acme-dashboard`),
      "open-agent-view",
      `Enter · launch ${providers[id].label}`,
    ));
  });
  frames.push(storyFrame(
    28.3,
    dashboard(providerOrder, "codex", `${cyan("name ❯")} migration-safety`, "rename session · enter save · esc cancel"),
    "open-agent-view",
    "Ctrl+R · rename session",
  ));
  frames.push(storyFrame(
    31,
    dashboard(providerOrder, "terminal", "describe a task · /help for commands", "Eight harnesses · one workspace · demo complete"),
    "open-agent-view",
    "Enter · save name",
  ));
  return { duration: 34, frames };
}

function harnessStory(id) {
  const provider = providers[id];
  if (id === "terminal") {
    return {
      duration: 20,
      frames: [
        storyFrame(0, picker(id), "open-agent-view", "/harness · choose Terminal"),
        storyFrame(2.5, dashboard([], "", `${provider.task}  ${dim("harness Terminal")}`), "open-agent-view", "Enter · select Terminal"),
        storyFrame(5, shell([`${cyan("Terminal")} · ${dim("/work/acme-dashboard")}`, "", `${prompt("$")} npm test`]), "Terminal", "Enter · open shell"),
        storyFrame(8, shell([`${cyan("Terminal")} · ${dim("/work/acme-dashboard")}`, "", `${prompt("$")} npm test`, green("✓ 7 tests passed"), "", `${prompt("$")} git status --short`]), "Terminal", "Enter · run tests"),
        storyFrame(12, shell([`${cyan("Terminal")} · ${dim("/work/acme-dashboard")}`, "", `${prompt("$")} npm test`, green("✓ 7 tests passed"), "", `${prompt("$")} git status --short`, dim("(clean)"), "", dim("← again to background this shell")]), "Terminal", "Enter · check status"),
        storyFrame(16, dashboard([id], id, "describe a task · /help for commands", "Terminal backgrounded; Enter resumes it"), "open-agent-view", "Shift+← · return"),
      ],
    };
  }
  return {
    duration: 22,
    frames: [
      storyFrame(0, picker(id), "open-agent-view", `/harness · choose ${provider.label}`),
      storyFrame(2.5, dashboard([], "", `${strong(provider.task)}  ${dim(`harness ${provider.label}`)}`), "open-agent-view", `Enter · select ${provider.label}`),
      storyFrame(5, nativeConversation(id, 0), provider.label, "Enter · launch session"),
      storyFrame(8, nativeConversation(id, 1), provider.label, "Enter · send task"),
      storyFrame(11, nativeConversation(id, 2), provider.label, "Enter · send follow-up"),
      storyFrame(14.5, nativeConversation(id, 3), provider.label, "Enter · send follow-up"),
      storyFrame(18.5, dashboard([id], id, "describe a task · /help for commands", `Returned from ${provider.label}; the session stays available`), "open-agent-view", "Shift+← · return"),
    ],
  };
}

const STORIES = {
  "story-overview": overviewStory(),
  "story-setup": {
    duration: 20,
    frames: [
      storyFrame(0, `${prompt("$")} curl -fsSL https://open-agent-view.github.io/install.sh | bash`, "Terminal", "Typed install command"),
      storyFrame(3, shell([`${prompt("$")} curl -fsSL https://open-agent-view.github.io/install.sh | bash`, dim("open-agent-view: downloading v0.1.35 for x86_64-unknown-linux-gnu")]), "Terminal", "Enter · install"),
      storyFrame(6, shell([`${prompt("$")} curl -fsSL https://open-agent-view.github.io/install.sh | bash`, green("open-agent-view: checksum verified"), green("open-agent-view: installed open-agent-view 0.1.35"), green("open-agent-view: installed shorthand: opav")]), "Terminal", "Installer completed"),
      storyFrame(9, shell([`${prompt("$")} opav`, dim("Opening Open Agent View in /work/acme-dashboard…")]), "Terminal", "Enter · launch opav"),
      storyFrame(11, dashboard([]), "open-agent-view", "Dashboard opened"),
      storyFrame(14, dashboard([], "", `${cyan("/harness")}`), "open-agent-view", "Typed /harness"),
      storyFrame(16.5, picker("claude"), "open-agent-view", "Enter · open picker"),
    ],
  },
  "story-rename": {
    duration: 16,
    frames: [
      storyFrame(0, dashboard(["claude", "codex", "pi", "opencode"], "codex"), "open-agent-view", "↓ · select Codex"),
      storyFrame(3, dashboard(["claude", "codex", "pi", "opencode"], "codex", `${cyan("name ❯")} database-migration`, "rename session · type a new name · enter save"), "open-agent-view", "Ctrl+R · rename"),
      storyFrame(6, dashboard(["claude", "codex", "pi", "opencode"], "codex", `${cyan("name ❯")} migration-safety`, "rename session · enter save · empty resets to provider name"), "open-agent-view", "Typed migration-safety"),
      storyFrame(9, dashboard(["claude", "codex", "pi", "opencode"], "codex", "describe a task · /help for commands", "Renamed locally to migration-safety; provider history is unchanged"), "open-agent-view", "Enter · save name"),
      storyFrame(12.5, dashboard(providerOrder, "codex", "describe a task · /help for commands", "Local display names remain stable across refreshes"), "open-agent-view", "Ctrl+L · refresh"),
    ],
  },
  "story-switch": {
    duration: 20,
    frames: [
      storyFrame(0, dashboard(providerOrder, "claude"), "open-agent-view", "↓ · select Claude Code"),
      storyFrame(3, nativeConversation("claude", 3), "Claude Code", "Enter · open session"),
      storyFrame(7, shell([nativeConversation("claude", 3), "", amber("Press ← again to go back to Open Agent View")]), "Claude Code", "← · arm return"),
      storyFrame(10, dashboard(providerOrder, "claude", "describe a task · /help for commands", "Returned without stopping Claude Code"), "open-agent-view", "← · return"),
      storyFrame(13, dashboard(providerOrder, "codex", "describe a task · /help for commands", "Shift+→ opens the selected native session immediately"), "open-agent-view", "↓ · select Codex"),
      storyFrame(16, nativeConversation("codex", 3), "OpenAI Codex", "Shift+→ · open session"),
    ],
  },
  "story-model": {
    duration: 18,
    frames: [
      storyFrame(0, dashboard(providerOrder, "pi", `${cyan("/model")}`), "open-agent-view", "Typed /model"),
      storyFrame(3, shell([strong("Choose Pi model · 6 results"), "", selected("› provider/default"), "  anthropic/sonnet", "  openai/gpt-5", "  google/gemini", "", dim("type to filter · ↑/↓ move · enter select · esc back")]), "open-agent-view", "Enter · open model picker"),
      storyFrame(7, shell([strong("Choose Pi model · 1 result"), "", `${dim("filter")}  sonnet`, selected("› anthropic/sonnet"), "", dim("enter uses the exact model ID")]), "open-agent-view", "Typed sonnet"),
      storyFrame(11, dashboard([], "", `${strong("Review the worker pool")}  ${dim("harness Pi · model anthropic/sonnet")}`), "open-agent-view", "Enter · select model"),
      storyFrame(14, nativeConversation("pi", 1), "Pi", "Enter · launch session"),
    ],
  },
  "story-login": {
    duration: 25,
    frames: [
      storyFrame(0, dashboard([], "", `${cyan("/login")}`), "open-agent-view", "Typed /login"),
      storyFrame(3, shell([strong("Harness setup · /work/acme-dashboard"), "", `${cyan("Claude Code")}     ${green("✓ installed")}  ${amber("sign in interactively")}`, `${cyan("OpenAI Codex")}    ${green("✓ installed")}  checking account…`, `${cyan("Pi")}              ${green("✓ installed")}  checking providers…`, `${cyan("OpenCode")}        ${green("✓ installed")}  checking providers…`, `${cyan("Cursor")}          ${green("✓ installed")}  checking account…`, `${cyan("GitHub Copilot")}  ${green("✓ installed")}  checking account…`, `${cyan("Antigravity")}     ${green("✓ installed")}  checking account…`]), "open-agent-view", "Enter · run setup"),
      storyFrame(7, shell([strong("Native login · Cursor"), "", "Open the browser link shown by Cursor.", dim("Authentication stays in the provider CLI; OAV never reads the token."), "", amber("Waiting for browser authentication…")]), "Cursor", "Enter · start native login"),
      storyFrame(11, shell([strong("Harness setup · /work/acme-dashboard"), "", `${cyan("Claude Code")}     ${green("✓ authenticated · models loaded")}`, `${cyan("OpenAI Codex")}    ${green("✓ authenticated · models loaded")}`, `${cyan("Pi")}              ${green("✓ providers available")}`, `${cyan("OpenCode")}        ${green("✓ providers available")}`, `${cyan("Cursor")}          ${green("✓ authenticated · models loaded")}`, `${cyan("GitHub Copilot")}  ${amber("sign in next")}`, `${cyan("Antigravity")}     ${amber("sign in next")}`]), "open-agent-view", "Shift+← · return"),
      storyFrame(15, shell([strong("Native login · GitHub Copilot"), "", `${prompt("$")} copilot login`, dim("Complete the device flow in your browser."), "", amber("Waiting for GitHub authentication…")]), "GitHub Copilot", "Enter · start native login"),
      storyFrame(19, shell([strong("Harness setup · complete"), "", `${green("✓")} Claude Code`, `${green("✓")} OpenAI Codex`, `${green("✓")} Pi`, `${green("✓")} OpenCode`, `${green("✓")} Cursor`, `${green("✓")} GitHub Copilot`, `${green("✓")} Antigravity`, "", dim("Terminal uses the local shell and needs no provider login.")]), "open-agent-view", "Shift+← · return"),
      storyFrame(22, picker("claude"), "open-agent-view", "/harness · open picker"),
    ],
  },
};

for (const id of providerOrder) STORIES[`story-${id}`] = harnessStory(id);

function formatTime(seconds) {
  const whole = Math.max(0, Math.floor(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}

class StoryPlayer {
  constructor(root) {
    this.root = root;
    this.screen = root.querySelector("[data-demo-screen]");
    this.progress = root.querySelector("[data-demo-progress]");
    this.pauseButton = root.querySelector('[data-demo-action="pause"]');
    this.windowLabel = root.querySelector("[data-demo-window]");
    this.lastAction = root.querySelector("[data-demo-last-action]");
    this.time = root.querySelector("[data-demo-time]");
    this.current = 0;
    this.playing = false;
    this.visible = false;
    this.lastTimestamp = 0;
    this.frameIndex = -1;
    this.ended = false;
    this.autoPaused = false;
    this.reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;
    this.load(root.dataset.story, false);
    this.bind();
  }

  load(id, play = true) {
    this.story = STORIES[id] || STORIES["story-overview"];
    this.root.dataset.story = id;
    this.current = 0;
    this.frameIndex = -1;
    this.ended = false;
    this.lastTimestamp = 0;
    this.render();
    if (play && !this.reducedMotion) this.play(false);
    else this.pause(false);
  }

  bind() {
    this.root.querySelector('[data-demo-action="back"]').addEventListener("click", () => this.seek(this.current - 5));
    this.root.querySelector('[data-demo-action="forward"]').addEventListener("click", () => this.seek(this.current + 5));
    this.root.querySelector('[data-demo-action="restart"]').addEventListener("click", () => { this.seek(0); this.play(); });
    this.pauseButton.addEventListener("click", () => this.playing ? this.pause() : this.play());
    this.progress.addEventListener("input", () => this.seek((Number(this.progress.value) / 1000) * this.story.duration));
    this.progress.addEventListener("change", () => this.root.dispatchEvent(new CustomEvent("demo-interaction", { bubbles: true })));
    for (const button of this.root.querySelectorAll("button")) {
      button.addEventListener("click", () => this.root.dispatchEvent(new CustomEvent("demo-interaction", { bubbles: true })));
    }
    this.observer = new IntersectionObserver(([entry]) => {
      const nextVisible = entry.isIntersecting && entry.intersectionRatio > .16;
      if (!nextVisible && this.visible && this.playing) {
        this.autoPaused = true;
        this.pause(false);
      }
      this.visible = nextVisible;
      if (this.visible && this.autoPaused && !this.ended) {
        this.autoPaused = false;
        this.play(false);
      } else if (this.visible && this.root.dataset.autoPlay === "true" && this.current === 0 && !this.reducedMotion) {
        this.play(false);
      }
    }, { threshold: [.16] });
    this.observer.observe(this.root);
  }

  play(user = true) {
    if (this.current >= this.story.duration) this.current = 0;
    this.ended = false;
    this.playing = true;
    this.lastTimestamp = 0;
    this.updateChrome();
    requestAnimationFrame((timestamp) => this.tick(timestamp));
    if (user) this.root.dispatchEvent(new CustomEvent("demo-interaction", { bubbles: true }));
  }

  pause(user = true) {
    this.playing = false;
    this.lastTimestamp = 0;
    this.updateChrome();
    if (user) this.root.dispatchEvent(new CustomEvent("demo-interaction", { bubbles: true }));
  }

  seek(value) {
    const wasEnded = this.ended;
    this.current = Math.max(0, Math.min(this.story.duration, value));
    this.ended = this.current >= this.story.duration;
    if (this.ended) this.playing = false;
    this.frameIndex = -1;
    this.render();
    if (this.ended && !wasEnded) {
      this.root.dispatchEvent(new CustomEvent("demo-ended", { bubbles: true }));
    }
  }

  tick(timestamp) {
    if (!this.playing) return;
    if (!this.lastTimestamp) this.lastTimestamp = timestamp;
    const delta = Math.min(.15, (timestamp - this.lastTimestamp) / 1000);
    this.lastTimestamp = timestamp;
    this.current = Math.min(this.story.duration, this.current + delta);
    this.render();
    if (this.current >= this.story.duration) {
      this.playing = false;
      this.ended = true;
      this.updateChrome();
      this.root.dispatchEvent(new CustomEvent("demo-ended", { bubbles: true }));
      return;
    }
    requestAnimationFrame((next) => this.tick(next));
  }

  render() {
    let nextIndex = 0;
    for (let index = 0; index < this.story.frames.length; index += 1) {
      if (this.story.frames[index].at <= this.current) nextIndex = index;
      else break;
    }
    if (nextIndex !== this.frameIndex) {
      this.frameIndex = nextIndex;
      const frame = this.story.frames[nextIndex];
      this.screen.innerHTML = frame.screen;
      this.windowLabel.textContent = frame.window;
      this.lastAction.textContent = frame.action;
      this.screen.scrollTop = this.screen.scrollHeight;
    }
    this.progress.value = String(Math.round((this.current / this.story.duration) * 1000));
    this.time.textContent = `${formatTime(this.current)} / ${formatTime(this.story.duration)}`;
    this.updateChrome();
  }

  updateChrome() {
    this.pauseButton.textContent = this.playing ? "Pause" : "Play";
    this.pauseButton.setAttribute("aria-label", this.playing ? "Pause demo" : "Resume demo");
  }
}

function initializeStories() {
  const players = new Map();
  for (const root of document.querySelectorAll("[data-demo-player]")) {
    players.set(root, new StoryPlayer(root));
  }

  for (const group of document.querySelectorAll("[data-tabbed-story]")) {
  const tabs = Array.from(group.querySelectorAll("[data-story-tab]"));
  const playerRoot = group.querySelector("[data-demo-player]");
  const player = players.get(playerRoot);
  const hold = group.querySelector("[data-tab-hold-progress]");
  let holdFrame = 0;
  let holdStarted = 0;

  const cancelHold = () => {
    cancelAnimationFrame(holdFrame);
    holdFrame = 0;
    holdStarted = 0;
    hold.style.width = "0%";
  };

  const select = (button, play = true) => {
    cancelHold();
    for (const tab of tabs) {
      const active = tab === button;
      tab.setAttribute("aria-selected", String(active));
      tab.tabIndex = active ? 0 : -1;
    }
    player.load(button.dataset.story, play);
  };

  const beginHold = () => {
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    cancelHold();
    const step = (timestamp) => {
      if (!holdStarted) holdStarted = timestamp;
      const ratio = Math.min(1, (timestamp - holdStarted) / 8000);
      hold.style.width = `${ratio * 100}%`;
      if (ratio >= 1) {
        const active = tabs.findIndex((tab) => tab.getAttribute("aria-selected") === "true");
        select(tabs[(active + 1) % tabs.length]);
        return;
      }
      holdFrame = requestAnimationFrame(step);
    };
    holdFrame = requestAnimationFrame(step);
  };

  for (const tab of tabs) {
    tab.addEventListener("click", () => select(tab));
    tab.addEventListener("keydown", (event) => {
      if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault();
      const index = tabs.indexOf(tab);
      const next = event.key === "Home" ? 0
        : event.key === "End" ? tabs.length - 1
          : event.key === "ArrowLeft" ? (index - 1 + tabs.length) % tabs.length
            : (index + 1) % tabs.length;
      tabs[next].focus();
      select(tabs[next]);
    });
  }
    group.addEventListener("demo-ended", beginHold);
    group.addEventListener("demo-interaction", cancelHold);
  }

  document.addEventListener("click", (event) => {
    const link = event.target.closest?.("[data-select-harness]");
    if (!link) return;
    const tab = document.querySelector(`#harness-demo [data-story-tab="${link.dataset.selectHarness}"]`);
    tab?.click();
  });
  document.documentElement.dataset.storiesReady = "true";
}

window.addEventListener("load", () => window.setTimeout(initializeStories, 750), { once: true });
