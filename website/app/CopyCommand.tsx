export function CopyCommand({ command, comment }: { command: string; comment?: string }) {
  return (
    <button
      className="copy-command"
      type="button"
      data-copy-command={command}
      suppressHydrationWarning
    >
      <code>
        <span>$</span> {command}
        {comment ? <i>  {comment}</i> : null}
      </code>
      <b aria-live="polite" suppressHydrationWarning>Copy</b>
    </button>
  );
}
