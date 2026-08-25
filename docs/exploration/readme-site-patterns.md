# README and homepage patterns

Reviewed 2026-08-25 from the projects' first-party repositories. This is a
design record, not a claim that Open Agent View is affiliated with any of them.

## Sources reviewed

- [astral-sh/uv](https://github.com/astral-sh/uv/blob/main/README.md) leads with
  one concrete sentence, a visual proof, a short highlights list, and a
  standalone installer before moving detailed examples below the fold.
- [jdx/mise](https://github.com/jdx/mise/blob/main/README.md) uses a centered
  identity block, a small navigation row, one real finite demo, a plain-language
  “What is it?”, and a quickstart whose output makes the result tangible.
- [sharkdp/bat](https://github.com/sharkdp/bat/blob/master/README.md) pairs each
  important capability with an actual screenshot instead of describing every
  option in the opening section.
- [zellij-org/zellij](https://github.com/zellij-org/zellij/blob/main/README.md)
  explains the product category and design philosophy before exposing advanced
  configuration, then points readers to dedicated screencasts and docs.
- [charmbracelet/gum](https://github.com/charmbracelet/gum/blob/main/README.md)
  begins with an authentic visual example and immediately turns it into a small,
  copyable tutorial.

## Patterns adopted

1. **Identity before inventory.** One product sentence, a small set of useful
   links, and the real demo appear before feature matrices or implementation
   notes.
2. **Proof next to promise.** The README demo is a finite recording assembled
   from the same genuine terminal captures used by the site. The site names
   every key press and keeps the provider's native UI visible.
3. **A two-command start.** Installation and launch are separate, copyable
   commands. Build-from-source details live in `docs/install.md`.
4. **Progressive detail.** The README describes the normal workflow and links
   to focused installation, keyboard, provider, architecture, safety, and test
   documents instead of reproducing them.
5. **Honest capability language.** The support table identifies the integration
   path without implying that every provider exposes the same controls.
6. **Readable motion.** Terminal stories play at 1×, shorten only genuine idle
   waits, pause on completed states, never loop, and retain the previous frame
   while seeking or changing tabs.

## Patterns deliberately avoided

- decorative badge walls that do not help someone install or evaluate OAV;
- feature-card repetition between the README and homepage;
- simulated terminal HTML presented as product evidence;
- autoplay loops that hide the final result;
- third-party logos without a clear first-party source or usage basis.
