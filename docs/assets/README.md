# Demo asset provenance

`website/public/open-agent-view-demo.mp4`, `open-agent-view.gif`, and
`open-agent-view.png` come from one genuine terminal recording made on
2026-08-28. It begins with eleven real coding-harness sessions
in the v0.1.48 dashboard, browses them with arrow keys, opens two other native
coding-harness TUIs, then opens Kimi Code, sends a lookup prompt, observes the
real response for seven seconds, and returns to the dashboard without waiting
for generation to finish. It does not generate HTML
rows that imitate a terminal. Key actions are burned from the separate action
manifest as a subtitle overlay; the recorded terminal bytes remain unchanged.

`ctrl-m-migration.gif` and `website/public/demos/migration.mp4` are focused
real-TUI renders made on 2026-08-31 from the same cast and action timeline.
The recording runs the release OAV binary, real Codex and Pi TUIs, and the
installed session-migrate CLI in a disposable home. It opens a genuine Codex
session, adds the code word `LANTERN`, performs the actual Codex-to-Pi
conversion, opens the imported Pi session, and asks it to recall the word.
Both renders burn the action captions into the frames and end on the dashboard
with the source and imported sessions visible.

`website/public/demos/migration.js` is a reusable browser module for the same
cast. It mounts the vendored Asciinema player and renders the action manifest
as an accessible subtitle overlay, so a future site embed does not simulate
terminal rows or provider responses.

The source casts live in [`website/public/demos`](../../website/public/demos).
Their exact recorded OAV version is declared in
[`version.json`](../../website/public/demos/version.json); release metadata is
never rewritten into terminal bytes after capture.
[`scripts/capture-real-site-demo.py`](../../scripts/capture-real-site-demo.py)
records the actual shell, OAV binary, and provider CLIs through private tmux and
Asciinema sessions. For the authenticated provider clips it copies the minimum
existing login state into a disposable mode-0700 home, removes it after capture,
and refuses to publish account email addresses, host identities, private paths,
or credential-like text. No credential value is printed or committed.

Recompose the README and website media from the reviewed real casts with:

```console
scripts/capture-site-demo.sh
```

Recapture the canonical overview and all state it genuinely depends on with:

```console
python3 scripts/capture-real-site-demo.py overview
```

Recapture and render the focused migration GIF and MP4 with:

```console
cargo build --release --locked
python3 scripts/capture-real-site-demo.py migration
python3 scripts/render-demo-gif.py migration docs/assets/ctrl-m-migration.gif \
  --mp4 website/public/demos/migration.mp4
```

The website publishes 22 real TUI recordings: the eleven-session overview,
install/setup, all sixteen selectable targets, and rename/switch/model/login controls. The sixteen target recordings
share one disposable workspace and show the exact picker, model, two-turn,
return, rename, and next-picker loop, preserving each earlier session row.
The control recordings add bottom-composer teaching text; rename uses a dense
normalized fixture, while switch/model/login exercise live managed shell, Pi,
and Claude paths respectively. The focused migration recording uses the same
real terminal recorder with authenticated, mode-0700 disposable provider
homes. The minimum login files are copied only for the capture and deleted
before the script exits; credentials never enter the cast, action timeline,
GIF, MP4, or JavaScript module.
Static tests parse every cast and action manifest, reject private material, and
browser tests mount the real vendored Asciinema player at desktop, Mac-laptop,
and phone sizes.
