# looper-terminal

A terminal interface (TUI) for communicating with Looper agents.

## Features

[ ] Provider API Key Management
[ ] Streaming Chat & Tasks
[ ] Model Selection
[ ] Session History
[ ] PEAS Plugin Management

## Commands

Each command is executed in the format: `/<command> <subcommand> <args>`

### `/agent`

Manage agents running on the system.

#### `/agent discover`

List all active agents on the system and their websocket ports

#### `/agent connect <port>`

Connect to an agent for chat and tasking

### `/provider`

Manage model provider details.

#### `/provider set <provider_id> <api_key>`

Add or replace the API key for a model provider.

#### `/provider unset <provider_id> <api_key>`

Removes the API key for a model provider (if exists).

### `/plugin`

Manage the PEAS plugins that are used by the agent.

#### `/plugin add <dir|url|zip>`

Adds a plugin to the agent.

#### `/plugin remove <plugin_id>`

Removes a plugin from the agent

#### `/soul`

Switch from the chat interface to the `SOUL.md` markdown.

#### `/skill <skill_id>`

Switch from the chat interface to the `SKILL.md` markdown.

### `/help`

List the details of all other commands.

## Keyboard Macros

### `ALT+SHIFT+C`

Switch from any non-chat interface back to the chat interface.

### `ALT+SHIFT+H`

View a popup and selection list of the session history.
