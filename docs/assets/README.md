# Demo asset provenance

`open-agent-view.gif` and `open-agent-view.png` are composed exclusively from
genuine terminal recordings made with the v0.1.35 release binary on
2026-08-25. The sequence shows the public installer and real OAV harness picker,
a short conversation in the real Claude Code TUI, the return to OAV, and an OAV
session rename. It does not generate HTML rows that imitate a terminal.

The source casts live in [`website/public/demos`](../../website/public/demos).
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

Recapture an individual source story explicitly—for example:

```console
python3 scripts/capture-real-site-demo.py setup
python3 scripts/capture-real-site-demo.py claude
python3 scripts/capture-real-site-demo.py rename
```

The website publishes 13 real recordings: install/setup, all eight selectable
targets, and rename/switch/model/login controls. Static tests parse every cast
and action manifest, reject private material, and browser tests mount the real
vendored Asciinema player at desktop and phone sizes.
