import assert from "node:assert/strict";
import { readFile, stat } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);
const escapeCharacter = String.fromCharCode(27);
const bellCharacter = String.fromCharCode(7);
const oscSequence = new RegExp(`${escapeCharacter}\\][\\s\\S]*?(?:${bellCharacter}|${escapeCharacter}\\\\)`, "g");
const csiSequence = new RegExp(`${escapeCharacter}\\[[0-?]*[ -/]*[@-~]`, "g");
const escapeRegExp = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

const demos = [
  ["setup", null, null],
  ["claude", "Claude Code", "Claude"],
  ["codex", "OpenAI Codex", "OpenAI Codex"],
  ["pi", "Pi", "Pi"],
  ["opencode", "OpenCode", "OpenCode"],
  ["cursor", "Cursor", "Cursor"],
  ["copilot", "GitHub Copilot", "GitHub Copilot"],
  ["antigravity", "Antigravity", "Antigravity"],
  ["mistral-vibe", "Mistral Vibe", "Mistral Vibe"],
  ["muse", "Muse Code", "Muse Code"],
  ["qwen", "Qwen Code", "Qwen Code"],
  ["kimi", "Kimi Code", "Kimi Code"],
  ["terminal", "Terminal", "Terminal"],
  ["overview", null, null],
  ["rename", null, null],
  ["switch", null, null],
  ["model", null, null],
  ["login", null, null],
];

const privateMaterial = [
  /(?:api[_-]?key|access[_-]?token|oauth[_-]?token|authorization\s*[:=]\s*bearer)/i,
  /(?:gh[pousr]_|sk-(?:proj-)?|AKIA)[A-Za-z0-9_-]{8,}/,
  /(?:^|[\s"'])\/(?:home|Users|tmp|private\/var)\//m,
  /(?:^|[\s"'])[A-Z]:\\Users\\/im,
  /(?:xlu41|@mcgill\.|@mila\.)/i,
];

async function render() {
  const workerUrl = new URL("../dist/server/index.js", import.meta.url);
  workerUrl.searchParams.set("test", `${process.pid}-${Date.now()}`);
  const { default: worker } = await import(workerUrl.href);

  return worker.fetch(
    new Request("https://open-agent-view.github.io/", {
      headers: { accept: "text/html" },
    }),
    { ASSETS: { fetch: async () => new Response("Not found", { status: 404 }) } },
    { waitUntil() {}, passThroughOnException() {} },
  );
}

function parseCast(source, name) {
  const lines = source.trim().split("\n");
  assert.ok(lines.length > 20, `${name}.cast should contain a real terminal timeline`);
  const header = JSON.parse(lines[0]);
  const events = lines.slice(1).map((line, index) => {
    const event = JSON.parse(line);
    assert.ok(Array.isArray(event), `${name}.cast event ${index + 1} should be an array`);
    assert.equal(event.length, 3, `${name}.cast event ${index + 1} should be cast v2-shaped`);
    return event;
  });
  return { header, events };
}

test("server-renders real recording controls, provider tabs, and canonical metadata", async () => {
  const response = await render();
  assert.equal(response.status, 200);
  assert.match(response.headers.get("content-type") ?? "", /^text\/html\b/i);

  const html = await response.text();
  assert.match(html, /<title>Open Agent View/);
  assert.match(html, /Monitor every agent/);
  assert.match(html, /Step in when it matters/);
  assert.match(html, /One live dashboard for 15 coding harnesses/);
  assert.match(html, /Eleven harnesses/);
  assert.match(html, /11 HARNESS SESSIONS · ONE DASHBOARD/);
  assert.match(html, /return without losing your place/);
  assert.match(html, /Choose any harness/);
  assert.match(html, /Work in its native CLI/);
  assert.match(html, /Manage every session/);
  assert.match(html, /From one dashboard/);
  assert.match(html, /One dashboard/);
  assert.match(html, /Native agent sessions/);
  assert.match(html, /View on GitHub/);
  assert.match(html, /Open the repository/);
  assert.match(html, /class="nav-github"/);
  assert.match(html, /class="external-arrow"/);
  assert.match(html, /opens in a new tab/);
  assert.match(html, /data-story="story-setup"/);
  assert.match(html, /data-story="story-overview"/);
  assert.match(html, /aria-label="INSTALL · OPEN · \/HARNESS playback controls"/);
  assert.match(html, /aria-label="Seek through INSTALL · OPEN · \/HARNESS"/);
  assert.match(html, /role="tablist" aria-label="Harness demos"/);
  assert.match(html, /data-demo-action="back"/);
  assert.match(html, /data-demo-action="pause"/);
  assert.match(html, /data-demo-action="forward"/);
  assert.match(html, /data-demo-action="restart"/);
  assert.match(html, /data-copy-command="open-agent-view"/);
  assert.match(html, /https:\/\/open-agent-view\.github\.io\/install\.sh/);
  assert.match(html, /rel="canonical" href="https:\/\/open-agent-view\.github\.io"/);
  assert.match(html, /property="og:image" content="https:\/\/open-agent-view\.github\.io\/og\.png"/);
  assert.match(html, /name="twitter:card" content="summary_large_image"/);
  assert.ok(
    (html.match(/href="https:\/\/github\.com\/xhluca\/open-agent-view"/g) ?? []).length >= 4,
    "GitHub should be prominent in the header, hero, repository banner, and footer",
  );
  assert.match(html, /href="#start">Demo<\/a>/);
  assert.match(html, /href="#install">Install<\/a>/);
  assert.ok(
    html.indexOf('id="start"') < html.indexOf('id="install"'),
    "the eleven-session overview should precede the standalone installer section",
  );
  assert.match(html, /class="story-action-subtitle"[^>]*data-demo-last-action[^>]*aria-atomic="true"/);
  assert.match(html, /href="https:\/\/github\.com\/xhluca\/open-agent-view" target="_blank" rel="noreferrer"/);

  for (const [id, label] of demos.slice(1, 13)) {
    assert.match(html, new RegExp(`data-story-tab="${id}"`));
    assert.match(html, new RegExp(`data-story="story-${id}"`));
    assert.match(html, new RegExp(`data-select-harness="${id}"`));
    assert.match(html, new RegExp(`Watch the ${label} demo`));
  }

  for (const label of ["Oh My Pi", "Grok", "Kilo Code", "OpenHands"]) {
    assert.match(html, new RegExp(`title="${label}"`));
  }

  assert.doesNotMatch(html, /codex-preview|Your site is taking shape|react-loading-skeleton/);
  assert.doesNotMatch(html, /Keep the conversation|Small commands|Fast context switches|One list|other tools|shared list/);
  assert.doesNotMatch(html, /raw\.githubusercontent\.com/);
  assert.doesNotMatch(html, /<video\b|data-demo-status|data-terminal-(?:row|grid|frame)/);
});

test("publishes genuine cast v2 recordings and action timelines for setup and every harness", async () => {
  const recordingMetadata = JSON.parse(
    await readFile(new URL("public/demos/version.json", root), "utf8"),
  );
  const recordingVersion = recordingMetadata.open_agent_view;
  assert.match(recordingVersion, /^\d+\.\d+\.\d+$/);
  assert.equal(recordingMetadata.kind, "real_terminal_recordings");

  const accumulatedSessionNames = [];
  for (const [name, , manifestName] of demos) {
    const [cast, actionsSource] = await Promise.all([
      readFile(new URL(`public/demos/${name}.cast`, root), "utf8"),
      readFile(new URL(`public/demos/${name}.actions.json`, root), "utf8"),
    ]);
    const { header, events } = parseCast(cast, name);
    const manifest = JSON.parse(actionsSource);
    const output = events.filter((event) => event[1] === "o").map((event) => event[2]).join("");
    const visibleOutput = output
      .replace(oscSequence, "")
      .replace(csiSequence, "")
      .replaceAll("\r", "");
    const finalTime = events.at(-1)[0];

    assert.equal(header.version, 2, `${name}.cast should use asciinema cast v2`);
    assert.ok(Number.isInteger(header.width) && header.width >= 80, `${name}.cast should record a useful width`);
    assert.ok(Number.isInteger(header.height) && header.height >= 24, `${name}.cast should record a useful height`);
    assert.ok(events.every((event) => Number.isFinite(event[0]) && ["o", "r"].includes(event[1]) && typeof event[2] === "string"));
    assert.ok(events.every((event, index) => index === 0 || event[0] >= events[index - 1][0]), `${name}.cast timestamps should be ordered`);
    assert.ok(output.length > 1_000, `${name}.cast should contain substantial real terminal output`);
    assert.ok(output.includes(`${escapeCharacter}[`), `${name}.cast should preserve terminal control sequences`);
    assert.ok(output.includes(`${escapeCharacter}[38;`), `${name}.cast should preserve native terminal colors`);
    const renderedVersions = [...output.matchAll(/Open Agent View v(\d+\.\d+\.\d+)/g)]
      .map((match) => match[1]);
    assert.ok(renderedVersions.length > 0, `${name}.cast should show the real application`);
    assert.deepEqual(
      [...new Set(renderedVersions)],
      [recordingVersion],
      `${name}.cast should contain only its declared real Open Agent View release`,
    );

    assert.ok(Number.isFinite(manifest.duration) && manifest.duration > 1);
    assert.ok(Math.abs(manifest.duration - finalTime) < 0.01, `${name} action duration should match its cast`);
    assert.ok(Array.isArray(manifest.actions) && manifest.actions.length > 1);
    assert.ok(manifest.actions.every((action, index) => (
      Number.isFinite(action.at)
      && action.at >= 0
      && action.at <= manifest.duration
      && (index === 0 || action.at >= manifest.actions[index - 1].at)
      && typeof action.action === "string"
      && action.action.length > 0
      && typeof action.window === "string"
      && action.window.length > 0
    )), `${name} actions should be ordered, bounded, and labelled`);

    if (name === "claude") {
      assert.equal(manifest.timing_adjustments?.length, 1);
      assert.equal(manifest.timing_adjustments[0].label, "Claude launch/background handoff");
      assert.ok(manifest.timing_adjustments[0].retimed_duration <= 1.8);
    } else if (name === "codex") {
      assert.equal(manifest.timing_adjustments?.length, 2);
      for (const adjustment of manifest.timing_adjustments) {
        assert.match(adjustment.label, /^Codex Working turn [12]$/);
        assert.ok(Math.abs(
          adjustment.retimed_duration / adjustment.source_duration - 0.4
        ) < 0.002, `${adjustment.label} should make 0.5× playback exactly 3× faster at 0.6×`);
      }
    } else {
      assert.equal(manifest.timing_adjustments, undefined);
    }

    if (name === "overview") {
      const overviewNames = [
        "claude-explanation", "codex-explanation", "pi-explanation",
        "opencode-explanation", "cursor-explanation", "copilot-explanation",
        "antigravity-explanation", "mistral-vibe-explanation",
        "muse-explanation", "qwen-explanation", "kimi-explanation",
      ];
      assert.equal(manifest.proof, "conversation");
      assert.equal(
        manifest.sequence,
        "eleven-session-dashboard-preview-two-open-kimi-lookup-return",
      );
      assert.equal(manifest.pacing_scale, 1.25);
      assert.deepEqual(manifest.preview_targets, ["qwen", "muse"]);
      assert.equal(manifest.preview_seconds, 2);
      assert.equal(manifest.lookup_seconds, 7);
      assert.equal(manifest.session_count, 11);
      assert.equal(manifest.target, "kimi");
      for (const sessionName of overviewNames) {
        if (sessionName === "qwen-explanation") {
          assert.match(visibleOutput, /qwen-explanation|Explain what is Qwen Code/);
        } else {
          assert.match(visibleOutput, new RegExp(escapeRegExp(sessionName)));
        }
      }
      assert.match(
        visibleOutput,
        /Look up https:\/\/open-agent-view\.github\.io\/ and tell me what it is about\./,
      );
      assert.ok(manifest.actions.some((action) => action.action === "↑ · choose Kimi Code"));
      assert.ok(manifest.actions.some((action) => action.action === "↓ · choose Qwen Code"));
      assert.ok(manifest.actions.some((action) => action.action === "↓ · choose Muse Code"));
      for (const provider of ["Qwen Code", "Muse Code", "Kimi Code"]) {
        assert.ok(manifest.actions.some((action) => action.action === `→ · open ${provider}`));
      }
      for (const provider of ["Qwen Code", "Muse Code"]) {
        const opened = manifest.actions.find((action) => action.action === `${provider} · native session`);
        const returned = manifest.actions.find((action) => (
          action.action === "Shift+← · return to dashboard" && action.at > opened?.at
        ));
        assert.ok(opened && returned);
        assert.ok(returned.at - opened.at >= 2, `${provider} should remain open for two seconds`);
      }
      const send = manifest.actions.find((action) => action.action === "Enter · send lookup prompt");
      const returned = manifest.actions.findLast(
        (action) => action.action === "Shift+← · return to dashboard",
      );
      assert.ok(send && returned);
      assert.ok(returned.at - send.at >= 7, "overview should observe the live response for seven seconds");
      assert.match(visibleOutput, /Open Agent View/);
    } else if (name === "setup") {
      assert.match(visibleOutput, /curl -fsSL https:\/\/open-agent-view\.github\.io\/install\.sh \| bash/);
      assert.match(visibleOutput, /\$ opav\b/);
      for (const choice of [
        "Claude", "Codex", "Pi", "OpenCode", "Cursor", "GitHub Copilot",
        "Antigravity", "Mistral Vibe", "Muse Code", "Qwen Code", "Kimi Code", "Terminal",
      ]) {
        // Full-screen terminal renders may position words with cursor movement rather
        // than literal spaces, so assert the complete label while tolerating that.
        const terminalLabel = choice.split(" ").map((part) => escapeRegExp(part)).join("\\s*");
        assert.match(visibleOutput, new RegExp(terminalLabel), `setup picker should show ${choice}`);
      }
    } else if (manifestName) {
      const initialFrameOutput = events
        .filter((event) => event[0] === 0 && event[1] === "o")
        .map((event) => event[2])
        .join("")
        .replace(oscSequence, "")
        .replace(csiSequence, "");
      assert.match(
        initialFrameOutput,
        /Open Agent View[\s\S]*choose harness/i,
        `${name} should inherit the prior demo's harness picker at time zero`,
      );
      assert.equal(manifest.sequence, "picker-model-two-turns-return-rename-picker");
      assert.equal(manifest.proof, name === "terminal" ? "terminal" : "conversation");
      assert.ok(
        manifest.actions.some((action) => `${action.action} ${action.window}`.includes(manifestName)),
        `${name} actions should identify ${manifestName}`,
      );
      assert.ok(
        manifest.actions.some((action) => (
          action.action === (name === "terminal" ? "Type /shell" : "Type /model")
        )),
        `${name} should visibly open its launch-option selection`,
      );
      if (name === "terminal") {
        assert.match(visibleOutput, /printf 'Hello from Terminal\.\\n'/);
        assert.match(visibleOutput, /Terminal is a real shell managed beside coding agents/);
        assert.doesNotMatch(visibleOutput, /\$ hello\b|\$ Explain\b/);
      }
      assert.ok(
        manifest.actions.some((action) => /(?:Explanation|Command).*complete/i.test(action.action)),
        `${name} should wait for the second turn to complete`,
      );
      assert.ok(
        manifest.actions.some((action) => action.action === "Shift+← · return to panel"),
        `${name} should return with Shift+Left`,
      );
      assert.ok(
        manifest.actions.some((action) => action.action === "Ctrl+R · rename session"),
        `${name} should rename the newly created row`,
      );
      assert.ok(
        manifest.actions.some((action) => action.action === "Type /harness"),
        `${name} should end back at the harness picker`,
      );
      assert.match(visibleOutput, new RegExp(`${escapeRegExp(name)}-explanation`));
      assert.match(visibleOutput, /choose harness/i);
      for (const priorName of accumulatedSessionNames) {
        assert.match(
          visibleOutput,
          new RegExp(escapeRegExp(priorName)),
          `${name} should preserve the earlier ${priorName} row`,
        );
      }
      accumulatedSessionNames.push(`${name}-explanation`);
      if (name === "qwen") {
        assert.doesNotMatch(
          visibleOutput,
          /API Error|unsupported parameter|max_tokens.*not supported/i,
          "Qwen's real conversation must complete without a model/API error",
        );
      }
    } else {
      assert.ok(
        manifest.actions.some((action) => action.window === "open-agent-view"),
        `${name} controls should visibly use the real Open Agent View TUI`,
      );
    }

    if (name === "rename") {
      assert.match(visibleOutput, /rename session/i);
      assert.match(visibleOutput, /release-review/i);
      assert.match(visibleOutput, /database-indexes/i);
      assert.match(visibleOutput, /launch-review/i);
      assert.match(visibleOutput, /api-cutover/i);
      assert.match(visibleOutput, /test-plan/i);
      assert.match(visibleOutput, /Claude/i);
      assert.match(visibleOutput, /Codex/i);
      assert.match(visibleOutput, /Pi/i);
      assert.equal(manifest.proof, "real-open-agent-view-tui");
      assert.equal(manifest.sequence, "guide-rename-three-multi-harness-sessions");
      assert.equal(
        manifest.actions.filter((action) => action.action === "Enter · save local name").length,
        3,
      );
      assert.ok(manifest.actions.some((action) => action.action === "Three renamed sessions visible"));
    }
    if (name === "switch") {
      assert.match(visibleOutput, /release-shell/i);
      assert.match(visibleOutput, /test-watcher/i);
      assert.match(visibleOutput, /api-server/i);
      assert.match(visibleOutput, /Press ← again/i);
      assert.equal(manifest.sequence, "guide-select-open-double-left-reopen-shift-left");
      assert.ok(manifest.actions.some((action) => action.action === "← again · return to dashboard"));
      assert.ok(manifest.actions.some((action) => action.action === "Shift+← · return immediately"));
    }
    if (name === "model") {
      assert.match(visibleOutput, /workspace-shell/i);
      assert.match(visibleOutput, /choose Pi model/i);
      assert.match(visibleOutput, /gpt-5\.4/i);
      assert.equal(manifest.sequence, "guide-browse-search-select-pi-model");
      assert.ok(manifest.actions.some((action) => action.action === "Type · search gpt-5.4"));
      assert.ok(manifest.actions.some((action) => action.action === "Enter · select the filtered model"));
    }
    if (name === "login") {
      assert.match(visibleOutput, /workspace-shell/i);
      assert.match(visibleOutput, /interactive login now\?/i);
      assert.match(visibleOutput, /Opening Claude Code login/i);
      assert.equal(manifest.sequence, "guide-check-open-native-login-background");
      assert.ok(manifest.actions.some((action) => action.action === "Native login ready"));
      assert.ok(manifest.actions.some((action) => action.action === "Returned without losing setup"));
    }

    for (const pattern of privateMaterial) {
      assert.doesNotMatch(`${cast}\n${actionsSource}`, pattern, `${name} must not publish secrets or private machine paths`);
    }
  }

  const readmeCast = await readFile(new URL("public/oav-demo.cast", root), "utf8");
  const overviewCast = await readFile(new URL("public/demos/overview.cast", root), "utf8");
  assert.equal(readmeCast, overviewCast, "README media should use the exact overview recording");
  const readmeVersions = [...readmeCast.matchAll(/Open Agent View v(\d+\.\d+\.\d+)/g)]
    .map((match) => match[1]);
  assert.ok(readmeVersions.length > 0, "README source cast should show Open Agent View");
  assert.deepEqual(
    [...new Set(readmeVersions)],
    [recordingVersion],
    "README media should contain only its declared real Open Agent View release",
  );
});

test("uses the local asciinema player without a synthetic terminal generator or playback loop", async () => {
  const [page, player, styles, script, playerBundle] = await Promise.all([
    readFile(new URL("app/page.tsx", root), "utf8"),
    readFile(new URL("app/DemoPlayer.tsx", root), "utf8"),
    readFile(new URL("app/globals.css", root), "utf8"),
    readFile(new URL("public/site.js", root), "utf8"),
    stat(new URL("public/asciinema-player.min.js", root)),
  ]);

  assert.ok(playerBundle.size > 50_000, "the local asciinema player bundle should be published");
  assert.match(script, /class RealCastPlayer/);
  assert.match(script, /AsciinemaPlayer\.create\(story\.cast/);
  assert.match(script, /autoPlay:\s*false/);
  assert.match(script, /const playbackFocus/);
  assert.match(script, /engaged:\s*false/);
  assert.match(script, /document\.visibilityState === "visible" && document\.hasFocus\(\)/);
  assert.match(player, /dispatchEvent\(new Event\("oav:react-hydrated"\)\)/);
  assert.match(script, /window\.addEventListener\("oav:react-hydrated", mountStories/);
  assert.match(script, /if \(!window\.__oavReactHydrated\)/);
  assert.match(script, /if \(document\.documentElement\.dataset\.storiesReady === "true"\) return/);
  assert.match(script, /loop:\s*false/);
  const playbackSpeeds = [...script.matchAll(/speed:\s*([0-9.]+)/g)].map((match) => Number(match[1]));
  assert.deepEqual(
    playbackSpeeds,
    [1, 1, ...Array(12).fill(0.6), ...Array(4).fill(1)],
    "overview, setup, and controls should stay literal while harness stories play at 0.6×",
  );
  assert.match(script, /retainFrame\(\)/);
  assert.match(script, /story-frame-cover/);
  assert.match(script, /this\.autoAdvance/);
  assert.match(script, /stalledAtEnd/);
  assert.match(script, /this\.ended = true/);
  assert.match(script, /this\.pauseButton\.textContent = "Replay"/);
  assert.doesNotMatch(script, /loop:\s*true|class StoryPlayer|syntheticFrames|terminalRows|renderTerminalFrame/);
  assert.doesNotMatch(page, /data-terminal-(?:row|grid|frame)|<video\b/);
  assert.doesNotMatch(player, /data-terminal-(?:row|grid|frame)|<video\b/);

  const tabUnderline = styles.match(/\.story-tabs button i\s*\{([^}]+)\}/s)?.[1] ?? "";
  assert.match(tabUnderline, /background:\s*var\(--cyan\)/);
  assert.match(styles, /\.story-tabs button\[aria-selected="true"\] i/);
  assert.doesNotMatch(page, /thin\s+cyan\s+line|counts down for eight seconds/i);
  assert.match(page, /Actual provider TUI output · complete turns · playback at 0.6×/);
  assert.doesNotMatch(`${page}\n${styles}\n${script}`, /data-tab-hold-progress|yellow hold bar|\.tab-hold/);

  const recorder = await readFile(new URL("../scripts/capture-real-site-demo.py", root), "utf8");
  assert.doesNotMatch(recorder, /["']NO_COLOR["']\s*:/, "capture must not disable provider colors");
  assert.match(recorder, /environment_file\s*=\s*root\s*\/\s*["']recording\.env["']/);
  assert.match(recorder, /environment_file\.chmod\(0o600\)/);
  assert.doesNotMatch(
    recorder,
    /inner_shell\s*=\s*f?["']env\s+\{?exports/,
    "capture credentials must not be serialized into the Asciinema process argv",
  );
  assert.match(recorder, /def prewarm_sequence_harnesses\(/);
  assert.match(recorder, /def prepare_cursor_demo_wrapper\(/);
  assert.ok(recorder.includes('--trust \\"$@\\"'));
  assert.match(recorder, /openai-codex\/gpt-5\.6-sol/);
  assert.match(recorder, /Error:\\s\*4\\d\\d/);
  assert.match(recorder, /SEQUENCE_PLAYBACK_SPEED\s*=\s*0\.5/);
  assert.match(recorder, /SEQUENCE_TYPING_SPEEDUP\s*=\s*0\.8/);
  assert.match(recorder, /CLAUDE_LAUNCH_TARGET_SECONDS\s*=\s*1\.8/);
  assert.match(recorder, /CODEX_WORKING_CAST_SCALE\s*=\s*0\.4/);
  assert.match(recorder, /sequence_wait\(1\.0\)[\s\S]{0,100}terminal\.key\("Enter"/);
  assert.match(recorder, /printf 'Hello from Terminal\.\\\\n'/);
  assert.doesNotMatch(recorder, /\.local["']?\s*\/\s*["']bin["']\s*\/\s*["'](?:hello|Explain)["']/);
});

test("keeps the public installer byte-identical to the application installer", async () => {
  const [source, published] = await Promise.all([
    readFile(new URL("../../install.sh", import.meta.url)),
    readFile(new URL("public/install.sh", root)),
  ]);
  assert.deepEqual(published, source);
});
