"use client";

import { useState } from "react";

import { CopyCommand } from "./CopyCommand";

const platforms = {
  unix: {
    label: "macOS / Linux",
    command: "curl -fsSL https://open-agent-view.github.io/install.sh | bash",
  },
  windows: {
    label: "Windows",
    command: "irm https://open-agent-view.github.io/install.ps1 | iex",
  },
} as const;

type Platform = keyof typeof platforms;

export function InstallCommandToggle() {
  const [platform, setPlatform] = useState<Platform>("unix");
  const selected = platforms[platform];

  return (
    <div className="install-command-switcher" data-install-command-switcher>
      <div className="install-platform-toggle" role="group" aria-label="Installation platform">
        {(Object.entries(platforms) as [Platform, (typeof platforms)[Platform]][]).map(
          ([id, option]) => (
            <button
              key={id}
              type="button"
              aria-pressed={platform === id}
              data-install-platform={id}
              onClick={() => setPlatform(id)}
            >
              {option.label}
            </button>
          ),
        )}
      </div>
      <CopyCommand command={selected.command} comment={`# ${selected.label}`} />
    </div>
  );
}
