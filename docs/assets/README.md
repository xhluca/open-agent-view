# Demo asset provenance

`website/public/open-agent-view-demo.mp4`, `open-agent-view.gif`, and
`open-agent-view.png` come from one genuine terminal recording made on
2026-08-27. It begins with eleven real coding-harness sessions
in the v0.1.41 dashboard, browses them with arrow keys, opens two other native
coding-harness TUIs, then opens Kimi Code, sends a lookup prompt, observes the
real response for seven seconds, and returns to the dashboard without waiting
for generation to finish. It does not generate HTML
rows that imitate a terminal. Key actions are burned from the separate action
manifest as a subtitle overlay; the recorded terminal bytes remain unchanged.

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

The website publishes 18 real TUI recordings: the eleven-session overview,
install/setup, all twelve selectable targets, and rename/switch/model/login controls. The twelve target recordings
share one disposable workspace and show the exact picker, model, two-turn,
return, rename, and next-picker loop, preserving each earlier session row.
The control recordings add bottom-composer teaching text; rename uses a dense
normalized fixture, while switch/model/login exercise live managed shell, Pi,
and Claude paths respectively.
Static tests parse every cast and action manifest, reject private material, and
browser tests mount the real vendored Asciinema player at desktop, Mac-laptop,
and phone sizes.
