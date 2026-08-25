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
      {/* Local provider marks avoid third-party requests at runtime. */}
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img src={provider.icon} alt="" />
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
      <span className="tab-hold" aria-hidden="true"><i data-tab-hold-progress /></span>
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
        <DemoPlayer
          story="story-overview"
          label="ONE WORKSPACE · EIGHT HARNESSES"
          caption="Launch · converse · return · rename · stop on the complete dashboard"
        />
      </section>

      <section className="guided section shell" id="start">
        <div className="section-heading">
          <div><p className="section-label">01 · START</p><h2>Install once.<br />Choose any harness.</h2></div>
          <p>
            The installer downloads a verified prebuilt binary. Launch with
            <code> opav</code>, type <code>/harness</code>, and the picker shows
            every available backend in the same workspace.
          </p>
        </div>
        <DemoPlayer
          story="story-setup"
          label="INSTALL · OPEN · /HARNESS"
          caption="A finite walkthrough—play, pause, seek, or restart it"
        />
      </section>

      <section className="guided section shell" id="harness-demo">
        <div className="section-heading">
          <div><p className="section-label">02 · WORK</p><h2>Pick a harness.<br />Keep the conversation.</h2></div>
          <p>
            Each tab starts from the same picker, launches a task, shows a
            short back-and-forth, then returns to the shared dashboard. Choose
            a logo above and this demo opens to that exact harness.
          </p>
        </div>
        <div className="tabbed-story" data-tabbed-story>
          <StoryTabs name="Harness demos" tabs={harnessTabs} initial="claude" />
          <DemoPlayer
            story="story-claude"
            label="HARNESS WALKTHROUGH"
            caption="A deterministic walkthrough of each provider-native handoff"
          />
        </div>
      </section>

      <section className="guided section shell" id="controls">
        <div className="section-heading">
          <div><p className="section-label">03 · CONTROL</p><h2>Small commands.<br />Fast context switches.</h2></div>
          <p>
            Rename locally, cross the native-session boundary, choose a model,
            or complete setup. When a story ends, its separate eight-second tab
            timer advances to the next control.
          </p>
        </div>
        <div className="tabbed-story" data-tabbed-story>
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
          <div><p className="section-label">TECHNICAL MODEL</p><h2>One index.<br />Native boundaries intact.</h2></div>
          <p>
            Open Agent View does not replace provider state. It normalizes
            verified observations into one index, grants controls only for
            sessions it can prove it owns, and hands foreground control back to
            the original CLI.
          </p>
        </div>
        <div className="architecture-map" aria-label="Open Agent View architecture">
          <div className="provider-stack">
            <span>Provider CLIs</span>
            <div>{providers.slice(0, 7).map((provider) => <ProviderMark key={provider.id} provider={provider} />)}</div>
            <small>Native history · auth · models · process state</small>
          </div>
          <div className="flow-arrow"><b>observe</b><i>→</i><em>documented APIs / verified files</em></div>
          <div className="oav-core">
            <span>Open Agent View</span>
            <strong>Session index</strong>
            <div><b>identity</b><b>state</b><b>capabilities</b><b>ownership</b></div>
            <small>Pre-indexed, paged, asynchronously refreshed</small>
          </div>
          <div className="flow-arrow return"><b>act</b><i>→</i><em>only when authority is exact</em></div>
          <div className="native-stack">
            <span>Native foreground</span>
            <strong>Exact session</strong>
            <small>Enter opens · ← twice returns · Shift+← returns immediately</small>
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
