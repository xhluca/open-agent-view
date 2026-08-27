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

function GitHubMark() {
  return (
    <svg
      aria-hidden="true"
      className="github-mark"
      viewBox="0 0 24 24"
      fill="currentColor"
    >
      <path d="M12 2C6.48 2 2 6.58 2 12.23c0 4.52 2.87 8.35 6.84 9.71.5.1.68-.22.68-.49 0-.24-.01-1.05-.02-1.91-2.78.62-3.37-1.21-3.37-1.21-.45-1.18-1.11-1.49-1.11-1.49-.91-.64.07-.63.07-.63 1 .08 1.53 1.06 1.53 1.06.9 1.56 2.34 1.11 2.91.85.09-.66.35-1.11.64-1.37-2.22-.26-4.56-1.14-4.56-5.07 0-1.12.39-2.04 1.03-2.76-.1-.26-.45-1.3.1-2.72 0 0 .84-.28 2.75 1.05A9.38 9.38 0 0 1 12 6.05c.85 0 1.69.12 2.49.34 1.91-1.33 2.75-1.05 2.75-1.05.55 1.42.2 2.46.1 2.72.64.72 1.03 1.64 1.03 2.76 0 3.94-2.34 4.8-4.57 5.06.36.32.68.94.68 1.89 0 1.37-.01 2.47-.01 2.81 0 .27.18.59.69.49A10.24 10.24 0 0 0 22 12.23C22 6.58 17.52 2 12 2Z" />
    </svg>
  );
}

function ExternalArrow() {
  return (
    <>
      <span className="external-arrow" aria-hidden="true">↗</span>
      <span className="sr-only"> (opens in a new tab)</span>
    </>
  );
}

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
          <div className="nav-internal" aria-label="On this page">
            <a href="#start">Demo</a>
            <a href="#install">Install</a>
            <a href="#harness-demo">Harnesses</a>
            <a href="#controls">Controls</a>
            <a href="#architecture">How it works</a>
          </div>
          <span className="nav-divider" aria-hidden="true" />
          <a
            className="nav-github"
            href="https://github.com/xhluca/open-agent-view"
            target="_blank"
            rel="noreferrer"
          >
            <GitHubMark />
            <span>GitHub</span>
            <ExternalArrow />
          </a>
        </div>
      </nav>

      <section className="hero shell" id="top">
        <p className="eyebrow"><span /> Local agents, one workspace</p>
        <h1><span>Monitor every agent.</span><span>Step in when it matters.</span></h1>
        <p className="hero-copy">
          One live dashboard for every coding harness. Jump into any native
          session, then return without losing your place.
        </p>
        <div className="hero-actions">
          <CopyCommand command={installCommand} />
          <CopyCommand command="open-agent-view" comment="# shorthand: opav" />
        </div>
        <div className="hero-resource-links">
          <a
            className="github-button"
            href="https://github.com/xhluca/open-agent-view"
            target="_blank"
            rel="noreferrer"
          >
            <GitHubMark />
            <span>View on GitHub</span>
            <ExternalArrow />
          </a>
          <a className="internal-button" href="#start">
            <span>Watch the dashboard demo</span>
            <span className="internal-arrow" aria-hidden="true">↓</span>
          </a>
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
          <div><p className="section-label">01 · OVERVIEW</p><h2>Eleven harnesses.<br />One live dashboard.</h2></div>
          <p>
            Move through every coding-agent session, open its native CLI, send
            new work, and return while it keeps running.
          </p>
        </div>
        <DemoPlayer
          story="story-overview"
          label="11 HARNESS SESSIONS · ONE DASHBOARD"
          caption="Real Open Agent View dashboard + native harness TUIs"
          actionPlacement="subtitle"
        />
      </section>

      <section className="guided section shell" id="install">
        <div className="section-heading">
          <div><p className="section-label">02 · INSTALL</p><h2>Install once.<br />Choose any harness.</h2></div>
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
          <div><p className="section-label">03 · WORK</p><h2>Choose any harness.<br />Work in its native CLI.</h2></div>
          <p>
            Start Claude Code, Codex, Pi, or another supported harness from the
            same workspace. Every session stays visible in Open Agent View while
            each conversation continues in the interface built for that harness.
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
            caption="Actual provider TUI output · complete turns · playback at 0.6×"
          />
        </div>
      </section>

      <section className="guided section shell" id="controls">
        <div className="section-heading">
          <div><p className="section-label">04 · CONTROL</p><h2>Manage every session.<br />From one dashboard.</h2></div>
          <p>
            Rename sessions, move between active agents, choose models, and
            complete sign-in or setup without losing track of the rest.
            Everything stays available from one dashboard.
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
            label="GUIDED CONTROLS"
            caption="Read the next step in the composer, perform it in the real TUI, and hold the result"
          />
        </div>
      </section>

      <section className="architecture section shell" id="architecture">
        <div className="section-heading">
          <div><p className="section-label">HOW IT WORKS</p><h2>One dashboard.<br />Native agent sessions.</h2></div>
          <p>
            Your conversations remain with the harness that created them—Claude
            Code, Codex, Pi, and the rest. Open Agent View brings those sessions
            into one dashboard. Open a session to return to its native CLI; go
            back to the dashboard and it keeps running.
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
            <small>Talk to the agent normally, then return to the shared dashboard without stopping it.</small>
          </div>
        </div>
        <a
          className="text-link external-text-link"
          href="https://github.com/xhluca/open-agent-view/blob/main/docs/control-model.md"
          target="_blank"
          rel="noreferrer"
        >
          Read the complete ownership and capability model <ExternalArrow />
        </a>
      </section>

      <aside className="repository-banner shell" aria-label="Open source repository">
        <div>
          <span>Open source</span>
          <strong>Inspect the code, follow development, or contribute.</strong>
        </div>
        <a
          className="github-button"
          href="https://github.com/xhluca/open-agent-view"
          target="_blank"
          rel="noreferrer"
        >
          <GitHubMark />
          <span>Open the repository</span>
          <ExternalArrow />
        </a>
      </aside>

      <footer className="footer shell">
        <a className="brand" href="#top"><span className="brand-mark" aria-hidden="true"><i /><i /></span><strong>Open Agent View</strong></a>
        <p>One control surface for all your local coding agents.</p>
        <div>
          <a href="https://github.com/xhluca/open-agent-view" target="_blank" rel="noreferrer">GitHub <ExternalArrow /></a>
          <a href="https://github.com/xhluca/open-agent-view/tree/main/docs" target="_blank" rel="noreferrer">Docs <ExternalArrow /></a>
        </div>
      </footer>
    </main>
  );
}
