# README design study

The project README was redesigned on 2026-08-18 after reviewing current README
files from established open-source command-line projects. These are product
presentation references, not implementation dependencies.

## Sources and lessons

- [uv](https://github.com/astral-sh/uv) leads with one sentence, a visual,
  outcome-oriented highlights, and a standalone installer that explicitly does
  not require the implementation language. Detailed workflows link to the
  documentation. **Applied:** state the value first, show the real TUI, and keep
  Rust out of the primary install path.
- [mise](https://github.com/jdx/mise) puts a compact navigation row, a demo, and
  a two-command quickstart before its deeper explanation. **Applied:** make the
  route from landing on the repository to running `coding-agents` obvious.
- [ripgrep](https://github.com/BurntSushi/ripgrep) precisely explains default
  behavior, shows real output, links to focused guides, and documents reasons
  not to use the tool. **Applied:** distinguish verified capabilities from
  observe-only boundaries and preview limitations.
- [GitHub CLI](https://github.com/cli/cli) keeps its introduction short, routes
  usage to a manual, separates platform installation, and documents release
  verification. **Applied:** move flags, lifecycle details, and release mechanics
  into `docs/`; retain a concise documentation map in the README.
- [Gum](https://github.com/charmbracelet/gum) demonstrates the product before
  enumerating commands and offers binaries/packages before a source-language
  install. **Applied:** use a real fixture-driven demo and treat source builds as
  a contributor path.

## Resulting README contract

The top-level README should answer, in order:

1. What is this and why would I use it?
2. What does the real interface look like?
3. How do I install and launch it?
4. Which providers and controls are actually supported?
5. Where are the operational, security, and contributor details?

Technical reference material belongs in the linked documentation unless it is
needed to make a safe first-run decision.
