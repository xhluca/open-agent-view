import { CopyCommand } from "./CopyCommand";
import { DemoPlayer } from "./DemoPlayer";

const installCommand = "curl -fsSL https://open-agent-view.github.io/install.sh | bash";

const providers = [
  { id: "claude", name: "Claude Code", icon: "/providers/claude.svg" },
  { id: "codex", name: "OpenAI Codex", icon: "/providers/codex.png" },
  { id: "pi", name: "Pi", icon: "/providers/pi.svg" },
  { id: "opencode", name: "OpenCode", icon: "/providers/opencode.svg" },
  { id: "cursor", name: "Cursor", icon: "/providers/cursor.svg" },
  { id: "copilot", name: "GitHub Copilot", icon: "/providers/copilot.svg" },
  { id: "antigravity", name: "Antigravity", icon: "/providers/antigravity.svg" },
  { id: "mistral-vibe", name: "Mistral Vibe", icon: "/providers/mistral-vibe.svg" },
  { id: "muse", name: "Muse Code", mark: "Mu" },
  { id: "qwen", name: "Qwen Code", icon: "/providers/qwen.svg" },
  { id: "kimi", name: "Kimi Code", mark: "K" },
  { id: "terminal", name: "Terminal", icon: "/providers/terminal.svg" },
] as const;

const controlTabs = [
  ["rename", "Rename"],
  ["switch", "Switch sessions"],
  ["model", "Model selection"],
  ["login", "Login & setup"],
] as const;

function ProviderMark({ provider }: { provider: (typeof providers)[number] }) {
  return (
    <span className={`provider-mark provider-${provider.id}`} aria-hidden="true">
      {"icon" in provider ? (
        <>
          {/* Local provider marks avoid third-party requests at runtime. */}
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={provider.icon} alt="" />
        </>
      ) : <b>{provider.mark}</b>}
    </span>
  );
}

function StoryTabs({
  name,
  tabs,
  initial,
}: {
  name: string;
  tabs: readonly (readonly [string, string])[];
  initial: string;
}) {
  return (
    <div className="story-tabs" role="tablist" aria-label={name} data-story-tabs>
      {tabs.map(([id, label]) => (
        <button
          key={id}
          type="button"
          role="tab"
          aria-selected={id === initial}
          tabIndex={id === initial ? 0 : -1}
          data-story-tab={id}
          data-story={`story-${id}`}
        >
          {label}
          <i aria-hidden="true" />
        </button>
      ))}
    </div>
  );
}

export default function Home() {
  const harnessTabs = providers.map(({ id, name }) => [id, name] as const);

  return (
    <main>
      <nav className="nav shell" aria-label="Main navigation">
        <a className="brand" href="#top" aria-label="Open Agent View home">
          <span className="brand-mark" aria-hidden="true"><i /><i /></span>
          <strong>Open Agent View</strong>
        </a>
        <div className="nav-links">
          <a href="#start">Start</a>
          <a href="#harness-demo">Harnesses</a>
          <a href="#controls">Controls</a>
          <a href="#architecture">Architecture</a>
          <a href="https://github.com/xhluca/open-agent-view">GitHub</a>
        </div>
      </nav>

      <section className="hero shell" id="top">
        <p className="eyebrow"><span /> Local agents, one workspace</p>
        <h1><span>Monitor every agent.</span><span>Step in when it matters.</span></h1>
        <p className="hero-copy">
          Launch, name, follow, and return to every local coding-agent session
          from one terminal—without losing the native harness.
        </p>
        <div className="hero-actions">
          <CopyCommand command={installCommand} />
          <CopyCommand command="open-agent-view" comment="# shorthand: opav" />
        </div>
        <div className="provider-row" aria-label="Choose a harness demo">
          {providers.map((provider) => (
            <a
              href="#harness-demo"
              data-select-harness={provider.id}
              aria-label={`Watch the ${provider.name} demo`}
              key={provider.id}
            >
              <ProviderMark provider={provider} />
              <span>{provider.name}</span>
            </a>
          ))}
        </div>
      </section>

      <section className="guided section shell" id="start">
        <div className="section-heading">
          <div><p className="section-label">01 · START</p><h2>Install once.<br />Choose any harness.</h2></div>
          <p>
            This is a real terminal recording: the installer is typed, the
            downloaded <code>opav</code> binary opens, and the actual harness
            picker is navigated with the arrow keys.
          </p>
        </div>
        <DemoPlayer
          story="story-setup"
          label="INSTALL · OPEN · /HARNESS"
          caption="Real shell + real Open Agent View TUI · recorded with tmux and asciinema"
        />
      </section>

      <section className="guided section shell" id="harness-demo">
        <div className="section-heading">
          <div><p className="section-label">02 · WORK</p><h2>Pick a harness.<br />Keep the conversation.</h2></div>
          <p>
            Each tab starts in the same real harness picker, chooses an available
            model (or Terminal shell), completes two turns in the native CLI, returns to the shared
            dashboard, renames the session, and leaves the picker ready for the
            next harness. Earlier sessions remain visible as the walkthrough grows.
          </p>
        </div>
        <div
          className="tabbed-story"
          data-tabbed-story
          data-auto-advance="true"
          data-auto-advance-delay="7000"
          data-auto-advance-loop="false"
        >
          <StoryTabs name="Harness demos" tabs={harnessTabs} initial="claude" />
          <DemoPlayer
            story="story-claude"
            label="HARNESS WALKTHROUGH"
            caption="Actual provider TUI output · complete turns · playback at 0.5×"
          />
        </div>
      </section>

      <section className="guided section shell" id="controls">
        <div className="section-heading">
          <div><p className="section-label">03 · CONTROL</p><h2>Small commands.<br />Fast context switches.</h2></div>
          <p>
            Rename a session, switch between the dashboard and a native CLI,
            choose a model, or complete setup. After a recording ends, the thin
            cyan line under its tab counts down for eight seconds, then opens
            the next example.
          </p>
        </div>
        <div
          className="tabbed-story"
          data-tabbed-story
          data-auto-advance="true"
          data-auto-advance-delay="8000"
          data-auto-advance-loop="false"
        >
          <StoryTabs name="Common control demos" tabs={controlTabs} initial="rename" />
          <DemoPlayer
            story="story-rename"
            label="EVERYDAY CONTROLS"
            caption="The story timeline and the next-tab countdown are independent"
          />
        </div>
      </section>

      <section className="architecture section shell" id="architecture">
        <div className="section-heading">
          <div><p className="section-label">HOW IT WORKS</p><h2>One list.<br />Your CLIs underneath.</h2></div>
          <p>
            Your conversations stay in Claude, Codex, Pi, and the other tools
            that created them. Open Agent View reads their session lists and
            puts them on one screen. When you open a session, you use that
            tool&apos;s native interface. When you go back, the task keeps running.
          </p>
        </div>
        <div className="architecture-map" aria-label="Open Agent View architecture">
          <div className="provider-stack">
            <span>Provider CLIs</span>
            <div>{providers.filter((provider) => provider.id !== "terminal").map((provider) => <ProviderMark key={provider.id} provider={provider} />)}</div>
            <small>Each CLI keeps its own login, models, and conversation history.</small>
          </div>
          <div className="flow-arrow"><b>read</b><i>→</i><em>session names, status, and recent activity</em></div>
          <div className="oav-core">
            <span>Open Agent View</span>
            <strong>One dashboard</strong>
            <div><b>which harness</b><b>waiting for input</b><b>what is running</b><b>what finished</b></div>
            <small>Fast navigation, filtering, naming, and model selection in one place.</small>
          </div>
          <div className="flow-arrow return"><b>open</b><i>→</i><em>enter the selected session in its original CLI</em></div>
          <div className="native-stack">
            <span>Original harness</span>
            <strong>Your session</strong>
            <small>Talk to the agent normally, then return to the shared list without stopping it.</small>
          </div>
        </div>
        <a className="text-link" href="https://github.com/xhluca/open-agent-view/blob/main/docs/control-model.md">Read the complete ownership and capability model ↗</a>
      </section>

      <footer className="footer shell">
        <a className="brand" href="#top"><span className="brand-mark" aria-hidden="true"><i /><i /></span><strong>Open Agent View</strong></a>
        <p>One control surface for all your local coding agents.</p>
        <div><a href="https://github.com/xhluca/open-agent-view">GitHub</a><a href="https://github.com/xhluca/open-agent-view/tree/main/docs">Docs</a></div>
      </footer>
    </main>
  );
}
