/**
 * Portable Ctrl+M demo for a future website embed.
 *
 * This module plays the same genuine Asciinema cast used to produce the GIF
 * and MP4. The action manifest is rendered as an accessible subtitle overlay;
 * no terminal rows or provider responses are synthesized in JavaScript.
 */

const asset = (name) => new URL(name, import.meta.url).href;

export const ctrlMMigrationDemoAssets = Object.freeze({
  cast: asset("migration.cast"),
  actions: asset("migration.actions.json"),
  mp4: asset("migration.mp4"),
});

function cueAt(actions, duration, time) {
  let selected = null;
  for (let index = 0; index < actions.length; index += 1) {
    const action = actions[index];
    if (action.at > time) break;
    const next = actions[index + 1];
    const end = Math.min(duration, next?.at ?? duration, action.at + 2.8);
    if (time <= end) selected = action;
  }
  return selected;
}

/**
 * Mount the real terminal recording and its action subtitles.
 *
 * The caller supplies the vendored AsciinemaPlayer global (or lets this use
 * `globalThis.AsciinemaPlayer`). The returned controller mirrors the small
 * playback surface needed by the current site and can be disposed cleanly.
 */
export async function mountCtrlMMigrationDemo(
  element,
  {
    playerLibrary = globalThis.AsciinemaPlayer,
    autoplay = false,
    speed = 1,
  } = {},
) {
  if (!(element instanceof Element)) {
    throw new TypeError("migration demo target must be a DOM element");
  }
  if (!playerLibrary?.create) {
    throw new Error("AsciinemaPlayer.create is required to mount the migration demo");
  }

  const response = await fetch(ctrlMMigrationDemoAssets.actions, { cache: "no-store" });
  if (!response.ok) {
    throw new Error(`migration actions returned ${response.status}`);
  }
  const manifest = await response.json();
  if (!Array.isArray(manifest.actions) || !Number.isFinite(manifest.duration)) {
    throw new Error("migration action manifest is invalid");
  }

  const frame = document.createElement("div");
  frame.className = "ctrl-m-migration-demo";
  frame.style.position = "relative";
  const terminal = document.createElement("div");
  terminal.className = "ctrl-m-migration-terminal";
  const subtitle = document.createElement("div");
  subtitle.className = "ctrl-m-migration-subtitle";
  subtitle.setAttribute("role", "status");
  subtitle.setAttribute("aria-live", "polite");
  subtitle.setAttribute("aria-atomic", "true");
  Object.assign(subtitle.style, {
    position: "absolute",
    left: "6%",
    right: "6%",
    bottom: "4%",
    padding: "0.55rem 0.8rem",
    color: "#f4f2dd",
    background: "rgba(26, 20, 8, 0.76)",
    border: "1px solid rgba(89, 194, 201, 0.5)",
    borderRadius: "0.45rem",
    font: "600 0.95rem/1.3 ui-monospace, SFMono-Regular, Menlo, monospace",
    textAlign: "center",
    opacity: "0",
    pointerEvents: "none",
  });
  frame.append(terminal, subtitle);
  element.replaceChildren(frame);

  const player = playerLibrary.create(ctrlMMigrationDemoAssets.cast, terminal, {
    autoPlay: autoplay,
    controls: true,
    cursorMode: "blinking",
    fit: "both",
    loop: false,
    speed,
    terminalFontFamily: "Geist Mono, ui-monospace, monospace",
    terminalFontSize: "18px",
    terminalLineHeight: 1.34,
    theme: "asciinema",
  });

  let disposed = false;
  let frameRequest = 0;
  let lastCue = "";
  const update = async () => {
    if (disposed) return;
    const time = await player.getCurrentTime();
    const cue = cueAt(manifest.actions, manifest.duration, time);
    const label = cue?.action ?? "";
    if (label !== lastCue) {
      lastCue = label;
      subtitle.textContent = label;
      subtitle.style.opacity = label ? "1" : "0";
    }
    frameRequest = requestAnimationFrame(update);
  };
  frameRequest = requestAnimationFrame(update);

  return Object.freeze({
    assets: ctrlMMigrationDemoAssets,
    manifest,
    player,
    play: () => player.play(),
    pause: () => player.pause(),
    seek: (time) => player.seek(time),
    dispose() {
      if (disposed) return;
      disposed = true;
      cancelAnimationFrame(frameRequest);
      player.dispose();
      frame.remove();
    },
  });
}
