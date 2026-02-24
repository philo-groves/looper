# Git Commit Guard Plugin

This example plugin exports:

- Sensor: `git_commits`
- Actuator: `desktop_notify_secrets`

The sensor watches local commits and emits percepts only for new risky commits.
If a new commit adds `.env` files or likely secrets (for example `token=` or `password=` in added lines),
it emits a risky percept.

Percepts are emitted as structured JSON with `looper_signal: "plugin_route_v1"` and
`route_to_actuator: "git_commit_guard:desktop_notify_secrets"` so Looper can route the action
deterministically.

The actuator scans recent commits and sends desktop notifications for risky commits
that have not been notified yet.
It only evaluates commits that are new since the previous actuator run.

## Import

From Looper API:

```bash
curl -X POST http://127.0.0.1:10001/api/plugins/import \
  -H "content-type: application/json" \
  -d '{"path":"examples/plugins/git-commit-guard"}'
```

## Notes

- The plugin expects Deno to be available on PATH.
- Notifications are best effort:
  - Windows: `powershell` (BurntToast if installed, otherwise `msg`)
  - macOS: `osascript`
  - Linux: `notify-send`
- Plugin state is stored in `.state.json` beside `mod.ts`.
- On first run, the plugin establishes a baseline and does not emit alerts for historical commits.
