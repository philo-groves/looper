# looper-discovery

A standalone discovery server to orchestrate the running of multiple Looper agents on a single system. This binary runs as a background process and uses websockets to communicate with agents.

## Features

- [x] Agent Port Assignments
- [x] Running Agent Statuses
- [x] Local Agent Lookup
- [x] Launch agents from persisted user config

## General Flow

When the discovery server starts, it is available via websocket at: ws://localhost:10001

At startup, discovery reads `%USERPROFILE%/.looper/agents.json` (or `$HOME/.looper/agents.json`) to build the local agent catalog.

Configured agents are not started automatically. They are started on-demand when requested by terminal.

After an agent starts, it reaches out to the discovery websocket for registration. The discovery server enumerates active agents and confirms the port to use for the agent websocket. This means each agent has two websockets: a server-side websocket for users, and a client-side websocket for discovery.

While an agent is active, it will retain its websocket connection to the discovery server, allowing agent lifetimes to be tracked easier.

When an agent completes setup, discovery persists that launch configuration back into `~/.looper/agents.json`.

## How to Build

`cargo build -p looper-discovery`

## How to Run

`cargo run -p looper-discovery`
