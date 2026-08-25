type DemoPlayerProps = {
  story: string;
  label: string;
  caption: string;
  autoPlay?: boolean;
};

export function DemoPlayer({ story, label, caption, autoPlay = true }: DemoPlayerProps) {
  return (
    <figure
      className="story-player"
      data-demo-player
      data-story={story}
      data-auto-play={autoPlay ? "true" : "false"}
    >
      <div className="story-topbar">
        <span><i /> {label}</span>
        <div className="story-controls" aria-label={`${label} playback controls`}>
          <button type="button" data-demo-action="back" aria-label="Go back five seconds">−5s</button>
          <button
            type="button"
            data-demo-action="pause"
            aria-label="Pause demo"
            suppressHydrationWarning
          >Pause</button>
          <button type="button" data-demo-action="forward" aria-label="Go forward five seconds">+5s</button>
          <button type="button" data-demo-action="restart" aria-label="Restart demo">Restart</button>
        </div>
      </div>
      <div className="story-window">
        <div className="window-bar">
          <strong data-demo-window suppressHydrationWarning>open-agent-view</strong>
          <em data-demo-last-action suppressHydrationWarning>Waiting</em>
        </div>
        <pre data-demo-screen aria-label={label} suppressHydrationWarning>Loading interactive terminal…</pre>
      </div>
      <label className="story-scrubber">
        <span className="sr-only">Demo position</span>
        <input
          type="range"
          min="0"
          max="1000"
          step="1"
          defaultValue="0"
          aria-label={`Seek through ${label}`}
          data-demo-progress
        />
      </label>
      <figcaption>
        <span>{caption}</span>
        <em data-demo-time suppressHydrationWarning>0:00 / 0:00</em>
      </figcaption>
    </figure>
  );
}
