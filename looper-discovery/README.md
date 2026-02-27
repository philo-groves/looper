# looper-discovery

A standalone discovery server to orchestrate the running of multiple Looper agents on a single system. This binary runs as a background process and uses websockets to communicate with agents.

## Features

- [ ] Agent Port Assignments
- [ ] Running Agent Statuses
- [ ] Local Agent Lookup

## General Flow

When the discovery server starts, it is available via websocket at: ws://localhost:10001

After an agent starts, it will reach out to the discovery server websocket for registration. The discovery server will enumerate all active agents, then return a port number for the agent to use for its own websocket. This means each agent has two websockets: a server-side websocket for users, and a client-side websocket for discovery.

While an agent is active, it will retain its websocket connection to the discovery server, allowing agent lifetimes to be tracked easier.

## How to Build

`cargo build -p looper-discovery`

## How to Run

`cargo run -p looper-discovery`