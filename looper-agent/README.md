# looper-agent

This binary crate is the core background process for each Looper agent instance.

## Startup Modes

- `running`: starts chat/task handling immediately when persisted settings already exist.
- `setup`: waits for terminal setup flow to provide workspace, provider, and API keys.

The first run stays in setup mode until these files are written inside the workspace directory:

- `settings.json`
- `keys.json`

## Arguments

`looper-agent` supports optional startup arguments:

- `--workspace-dir <path>`
- `--port <port>`

If omitted, discovery assigns a port and the agent can be configured from terminal setup mode.

## Features

- [ ] Chat Interaction
- [ ] Multi-model Functionality
- [ ] Task Planning & Execution
- [ ] Memory Management
- [ ] PEAS Orchestration
- [ ] World Change Tracking
- [ ] Operational Observability
- [ ] Websocket Connections

Additional feature lists are available the libraries:
- [looper-peas](../looper-peas/README.md)
- [looper-settings](../looper-settings/README.md)

### About the Process


