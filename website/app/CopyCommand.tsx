export function CopyCommand({ command }: { command: string }) {
  return (
    <button className="copy-command" type="button" data-copy-command={command}>
      <code><span>$</span> {command}</code>
      <b aria-live="polite" suppressHydrationWarning>Copy</b>
    </button>
  );
}
