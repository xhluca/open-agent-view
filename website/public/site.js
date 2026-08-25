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

const STORIES = {
  "story-setup": {
    cast: "/demos/setup.cast",
    actions: "/demos/setup.actions.json",
    speed: 1.45,
  },
  "story-claude": {
    cast: "/demos/claude.cast",
    actions: "/demos/claude.actions.json",
    speed: 1.5,
  },
  "story-codex": {
    cast: "/demos/codex.cast",
    actions: "/demos/codex.actions.json",
    speed: 1.5,
  },
  "story-pi": {
    cast: "/demos/pi.cast",
    actions: "/demos/pi.actions.json",
    speed: 1.5,
  },
  "story-opencode": {
    cast: "/demos/opencode.cast",
    actions: "/demos/opencode.actions.json",
    speed: 1.5,
  },
  "story-cursor": {
    cast: "/demos/cursor.cast",
    actions: "/demos/cursor.actions.json",
    speed: 1.5,
  },
  "story-copilot": {
    cast: "/demos/copilot.cast",
    actions: "/demos/copilot.actions.json",
    speed: 1.5,
  },
  "story-antigravity": {
    cast: "/demos/antigravity.cast",
    actions: "/demos/antigravity.actions.json",
    speed: 1.5,
  },
  "story-terminal": {
    cast: "/demos/terminal.cast",
    actions: "/demos/terminal.actions.json",
    speed: 1.5,
  },
  "story-rename": {
    cast: "/demos/rename.cast",
    actions: "/demos/rename.actions.json",
    speed: 1.25,
  },
  "story-switch": {
    cast: "/demos/switch.cast",
    actions: "/demos/switch.actions.json",
    speed: 1.25,
  },
  "story-model": {
    cast: "/demos/model.cast",
    actions: "/demos/model.actions.json",
    speed: 1.25,
  },
  "story-login": {
    cast: "/demos/login.cast",
    actions: "/demos/login.actions.json",
    speed: 1.25,
  },
};

function formatTime(seconds) {
  const whole = Math.max(0, Math.floor(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}

function safe(action) {
  try { return action(); } catch { return undefined; }
}

class RealCastPlayer {
  constructor(root) {
    this.root = root;
    this.mount = root.querySelector("[data-demo-screen]");
    this.progress = root.querySelector("[data-demo-progress]");
    this.pauseButton = root.querySelector('[data-demo-action="pause"]');
    this.windowLabel = root.querySelector("[data-demo-window]");
    this.lastAction = root.querySelector("[data-demo-last-action]");
    this.timeLabel = root.querySelector("[data-demo-time]");
    this.player = null;
    this.manifest = null;
    this.playing = false;
    this.visible = true;
    this.ended = false;
    this.generation = 0;
    this.reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    for (const button of root.querySelectorAll("[data-demo-action]")) {
      button.addEventListener("click", () => this.control(button.dataset.demoAction));
    }
    this.progress.addEventListener("pointerdown", () => this.pause());
    this.progress.addEventListener("input", () => {
      if (!this.manifest || !this.player) return;
      const seconds = Number(this.progress.value) / 1000 * this.manifest.duration;
      safe(() => this.player.seek(seconds));
      this.ended = seconds >= this.manifest.duration - 0.05;
      this.update(seconds);
    });

    this.observer = new IntersectionObserver(([entry]) => {
      this.visible = entry.isIntersecting;
      if (!this.player) return;
      if (this.visible && this.playing) safe(() => this.player.play());
      else safe(() => this.player.pause());
    }, { threshold: 0.18 });
    this.observer.observe(root);
    this.mountStory(root.dataset.story);
    window.requestAnimationFrame(() => this.tick());
  }

  async mountStory(storyId) {
    const story = STORIES[storyId];
    const generation = ++this.generation;
    this.root.dataset.story = storyId;
    safe(() => this.player?.dispose());
    this.player = null;
    this.manifest = null;
    this.playing = false;
    this.ended = false;
    this.progress.value = "0";
    this.mount.replaceChildren();

    if (!story) {
      const unavailable = document.createElement("p");
      unavailable.className = "recording-unavailable";
      unavailable.textContent = "This real native recording has not been published yet.";
      this.mount.append(unavailable);
      this.windowLabel.textContent = "recording unavailable";
      this.lastAction.textContent = "No simulated terminal is shown";
      this.timeLabel.textContent = "—";
      this.pauseButton.textContent = "Play";
      return;
    }

    try {
      const response = await fetch(story.actions, { cache: "no-store" });
      if (!response.ok) throw new Error(`actions returned ${response.status}`);
      const manifest = await response.json();
      if (generation !== this.generation) return;
      this.manifest = manifest;
      this.player = window.AsciinemaPlayer.create(story.cast, this.mount, {
        autoPlay: !this.reducedMotion && this.root.dataset.autoPlay === "true",
        controls: false,
        cursorMode: "blinking",
        fit: "width",
        idleTimeLimit: 1,
        loop: false,
        speed: story.speed,
        theme: "asciinema",
        terminalFontFamily: "Geist Mono, ui-monospace, monospace",
        terminalLineHeight: 1.34,
      });
      this.playing = !this.reducedMotion && this.root.dataset.autoPlay === "true";
      this.pauseButton.textContent = this.playing ? "Pause" : "Play";
      this.player.addEventListener("ended", () => this.finish());
      this.update(0);
    } catch (error) {
      if (generation !== this.generation) return;
      const failure = document.createElement("p");
      failure.className = "recording-unavailable";
      failure.textContent = `Could not load the real terminal recording: ${error.message}`;
      this.mount.replaceChildren(failure);
    }
  }

  currentTime() {
    return Number(safe(() => this.player?.getCurrentTime()) || 0);
  }

  control(action) {
    if (!this.player || !this.manifest) return;
    if (action === "pause") {
      if (this.playing) this.pause();
      else this.play();
      return;
    }
    if (action === "restart") {
      safe(() => this.player.seek(0));
      this.ended = false;
      this.play();
      this.update(0);
      return;
    }
    const delta = action === "back" ? -5 : 5;
    const next = Math.max(0, Math.min(this.manifest.duration, this.currentTime() + delta));
    safe(() => this.player.seek(next));
    this.ended = next >= this.manifest.duration - 0.05;
    this.update(next);
  }

  play() {
    if (!this.player) return;
    if (this.ended) safe(() => this.player.seek(0));
    this.ended = false;
    this.playing = true;
    this.pauseButton.textContent = "Pause";
    if (this.visible) safe(() => this.player.play());
  }

  pause() {
    this.playing = false;
    this.pauseButton.textContent = "Play";
    safe(() => this.player?.pause());
  }

  finish() {
    if (this.ended) return;
    this.ended = true;
    this.playing = false;
    this.pauseButton.textContent = "Replay";
    this.update(this.manifest?.duration || this.currentTime());
    this.root.dispatchEvent(new CustomEvent("demo-ended", { bubbles: true }));
  }

  update(seconds) {
    if (!this.manifest) return;
    const bounded = Math.max(0, Math.min(seconds, this.manifest.duration));
    const action = [...this.manifest.actions]
      .reverse()
      .find((candidate) => candidate.at <= bounded)
      || this.manifest.actions[0];
    this.windowLabel.textContent = action?.window || "Terminal";
    this.lastAction.textContent = action?.action || "Ready";
    this.progress.value = String(Math.round(bounded / this.manifest.duration * 1000));
    this.timeLabel.textContent = `${formatTime(bounded)} / ${formatTime(this.manifest.duration)}`;
  }

  tick() {
    if (this.player && this.manifest) {
      const current = this.currentTime();
      this.update(current);
      if (!this.ended && current >= this.manifest.duration - 0.05) this.finish();
    }
    window.requestAnimationFrame(() => this.tick());
  }
}

class TabbedStory {
  constructor(root) {
    this.root = root;
    this.tabs = [...root.querySelectorAll('[role="tab"]')];
    this.player = root.querySelector("[data-demo-player]")._realCastPlayer;
    this.holdFrame = 0;
    this.holdStarted = 0;
    this.tabs.forEach((tab, index) => {
      tab.addEventListener("click", () => this.select(index));
      tab.addEventListener("keydown", (event) => {
        if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
        event.preventDefault();
        let next = index;
        if (event.key === "ArrowLeft") next = (index - 1 + this.tabs.length) % this.tabs.length;
        if (event.key === "ArrowRight") next = (index + 1) % this.tabs.length;
        if (event.key === "Home") next = 0;
        if (event.key === "End") next = this.tabs.length - 1;
        this.select(next, true);
      });
    });
    root.addEventListener("demo-ended", () => {
      if (root.closest("#controls")) this.startHold();
    });
  }

  selectedIndex() {
    return Math.max(0, this.tabs.findIndex((tab) => tab.getAttribute("aria-selected") === "true"));
  }

  select(index, focus = false) {
    window.cancelAnimationFrame(this.holdFrame);
    this.holdFrame = 0;
    for (const [tabIndex, tab] of this.tabs.entries()) {
      const selected = tabIndex === index;
      tab.setAttribute("aria-selected", String(selected));
      tab.tabIndex = selected ? 0 : -1;
      tab.style.setProperty("--tab-progress", selected ? "1" : "0");
    }
    if (focus) this.tabs[index].focus();
    this.player.mountStory(this.tabs[index].dataset.story);
  }

  startHold() {
    window.cancelAnimationFrame(this.holdFrame);
    const tab = this.tabs[this.selectedIndex()];
    this.holdStarted = performance.now();
    const advance = (now) => {
      const elapsed = now - this.holdStarted;
      const remaining = Math.max(0, 1 - elapsed / 8000);
      tab.style.setProperty("--tab-progress", String(remaining));
      if (remaining > 0) this.holdFrame = window.requestAnimationFrame(advance);
      else this.select((this.selectedIndex() + 1) % this.tabs.length);
    };
    this.holdFrame = window.requestAnimationFrame(advance);
  }
}

function mountStories() {
  if (document.documentElement.dataset.storiesReady === "true") return;
  if (!window.__oavReactHydrated) {
    window.setTimeout(mountStories, 50);
    return;
  }
  if (!window.AsciinemaPlayer) {
    window.setTimeout(mountStories, 50);
    return;
  }
  for (const root of document.querySelectorAll("[data-demo-player]")) {
    root._realCastPlayer = new RealCastPlayer(root);
  }
  for (const root of document.querySelectorAll("[data-tabbed-story]")) {
    root._tabbedStory = new TabbedStory(root);
  }
  for (const link of document.querySelectorAll("[data-select-harness]")) {
    link.addEventListener("click", () => {
      const story = document.querySelector("#harness-demo [data-tabbed-story]");
      const index = story._tabbedStory.tabs.findIndex(
        (tab) => tab.dataset.storyTab === link.dataset.selectHarness,
      );
      if (index >= 0) story._tabbedStory.select(index);
    });
  }
  document.documentElement.dataset.storiesReady = "true";
  enableCopyCommands();
}

window.addEventListener("load", mountStories, { once: true });
window.addEventListener("oav:react-hydrated", mountStories, { once: true });
