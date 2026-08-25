import { CopyCommand } from "./CopyCommand";

const installCommand = "curl -fsSL https://open-agent-view.github.io/install.sh | bash";

const providers = [
  { name: "Claude Code", scope: "Managed + native", actions: "Launch · attach · stop" },
  { name: "OpenAI Codex", scope: "Managed", actions: "Reply · approve · interrupt · archive" },
  { name: "Pi", scope: "Managed + native", actions: "Reply · input · stop · delete" },
  { name: "OpenCode", scope: "Managed + history", actions: "Inspect · reply · interrupt" },
  { name: "Cursor", scope: "OAV-owned", actions: "Models · launch · resume · interrupt" },
  { name: "GitHub Copilot", scope: "OAV-owned + ACP", actions: "Reply · cancel · one-shot approval" },
  { name: "Antigravity", scope: "OAV-owned", actions: "Models · launch · resume · stop" },
  { name: "Terminal", scope: "OAV-owned", actions: "Background · resume · stop · delete" },
];

const workflow = [
  ["01", "Discover", "See managed work across every enabled harness in one responsive queue."],
  ["02", "Follow", "Read current state, recent output, and requests for input without terminal hopping."],
  ["03", "Intervene", "Reply, approve, interrupt, or archive only where the provider safely supports it."],
  ["04", "Return", "Open the exact native session with its history and ownership intact."],
] as const;

export default function Home() {
  return (
    <main>
      <nav className="nav shell" aria-label="Main navigation">
        <a className="brand" href="#top" aria-label="Open Agent View home">
          <span className="brand-mark" aria-hidden="true"><i /><i /></span>
          <strong>Open Agent View</strong>
        </a>
        <div className="nav-links">
          <a href="#demo">Demo</a>
          <a href="#workflow">Workflow</a>
          <a href="#harnesses">Harnesses</a>
          <a href="https://github.com/xhluca/open-agent-view">GitHub</a>
        </div>
      </nav>

      <section className="hero shell" id="top">
        <p className="eyebrow"><span /> Local agents, one queue</p>
        <h1>See every agent.<br />Step in when it matters.</h1>
        <p className="hero-copy">
          Launch, follow, and safely control your local coding-agent sessions
          from one terminal—then return to the native harness whenever you want.
        </p>
        <div className="hero-actions">
          <CopyCommand command={installCommand} />
          <a className="primary-link" href="https://github.com/xhluca/open-agent-view">
            View on GitHub <span>↗</span>
          </a>
        </div>
        <div className="provider-row" aria-label="Supported harnesses">
          {providers.map(({ name }) => <span key={name}>{name}</span>)}
        </div>
        <figure className="product-frame">
          <div className="window-bar"><i /><i /><i /><span>open-agent-view</span><em>LIVE</em></div>
          {/* A plain image keeps the GitHub Pages export independent of a
              server-side Next image optimizer. */}
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img
            src="/open-agent-view.png"
            width={1190}
            height={784}
            alt="Open Agent View showing local coding-agent sessions grouped by status"
          />
        </figure>
      </section>

      <section className="demo section shell" id="demo">
        <div className="section-heading">
          <div><p className="section-label">REAL TUI · FRESH CONTAINER</p><h2>One loop.<br />Every harness.</h2></div>
          <p>
            A real Open Agent View binary discovers concurrent work, navigates
            the queue, reveals contextual controls, and switches grouping views
            without contacting any provider.
          </p>
        </div>
        <figure className="demo-frame">
          <video controls muted preload="metadata" poster="/oav-demo.png">
            <source src="/oav-demo.mp4" type="video/mp4" />
            <track kind="captions" src="/oav-demo.vtt" srcLang="en" label="English" default />
            Your browser does not support embedded video.
          </video>
          <figcaption><span><i /> Recorded from an isolated Docker environment</span><a href="https://github.com/xhluca/open-agent-view/blob/main/docs/tui-validation.md">Reproduce it ↗</a></figcaption>
        </figure>
      </section>

      <section className="workflow section shell" id="workflow">
        <p className="section-label">THE SUPERVISOR LOOP</p>
        <h2>Run more agents.<br />Lose less context.</h2>
        <div className="workflow-grid">
          {workflow.map(([number, title, copy]) => (
            <article key={number}><b>{number}</b><h3>{title}</h3><p>{copy}</p></article>
          ))}
        </div>
      </section>

      <section className="harnesses section shell" id="harnesses">
        <div className="section-heading">
          <div><p className="section-label">CAPABILITY-AWARE BY DESIGN</p><h2>The right controls.<br />Never pretend controls.</h2></div>
          <p>
            Every integration declares what it can safely do. Unsupported
            actions stay unavailable instead of being guessed or simulated.
          </p>
        </div>
        <div className="capability-grid" aria-label="Supported harness capabilities">
          {providers.map(({ name, scope, actions }) => (
            <article key={name}>
              <i aria-hidden="true" />
              <div><h3>{name}</h3><p>{scope}</p><span>{actions}</span></div>
            </article>
          ))}
        </div>
        <a className="text-link" href="https://github.com/xhluca/open-agent-view/blob/main/docs/control-model.md">Read the exact ownership and capability model ↗</a>
      </section>

      <section className="ownership section shell">
        <div className="ownership-copy">
          <p className="section-label">NATIVE OWNERSHIP</p>
          <h2>Your sessions stay yours.</h2>
          <p>
            Open Agent View coordinates through verified provider boundaries.
            The native harness remains authoritative, external history is
            opt-in, and mutating actions fail closed when ownership is unclear.
          </p>
        </div>
        <div className="ownership-flow" aria-label="Native ownership flow">
          <div><span>01</span><strong>Native harness</strong><small>Source of truth</small></div>
          <i>↓ observe</i>
          <div className="active"><span>02</span><strong>Open Agent View</strong><small>Capability-gated control</small></div>
          <i>↓ explicit action</i>
          <div><span>03</span><strong>Native harness</strong><small>History stays intact</small></div>
        </div>
      </section>

      <section className="install section shell" id="install">
        <div>
          <p className="section-label">INSTALL</p>
          <h2>One command.<br />Then keep moving.</h2>
          <p>Open Agent View installs as a prebuilt binary. No Rust or Cargo required.</p>
        </div>
        <div className="install-panel">
          <span>Linux x86-64 · private preview</span>
          <CopyCommand command={installCommand} />
          <code>open-agent-view</code>
          <small>The installer uses your existing GitHub authentication for the private release and verifies its checksum.</small>
        </div>
      </section>

      <section className="faq section shell" id="faq">
        <p className="section-label">COMMON QUESTIONS</p>
        <h2>Built to stay honest.</h2>
        <details><summary>Does Open Agent View replace the native CLIs?</summary><p>No. It gives you a shared queue and verified controls, then opens the exact native session when you want its full interface.</p></details>
        <details><summary>Does it scan every conversation on my machine?</summary><p>No. The default queue contains OAV-managed work. Provider-wide history requires the explicit <code>--include-external</code> flag.</p></details>
        <details><summary>Why are controls different across harnesses?</summary><p>Providers expose different documented surfaces. OAV shows only the actions it can safely verify for the selected session.</p></details>
      </section>

      <footer className="footer shell">
        <a className="brand" href="#top"><span className="brand-mark" aria-hidden="true"><i /><i /></span><strong>Open Agent View</strong></a>
        <p>One control surface for all your local coding agents.</p>
        <div><a href="https://github.com/xhluca/open-agent-view">GitHub</a><a href="https://github.com/xhluca/open-agent-view/tree/main/docs">Docs</a></div>
      </footer>
    </main>
  );
}
