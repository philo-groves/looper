# PEAS

This project contains the core functionality for Looper agents to load and run PEAS plugins. If you are not familiar with PEAS concepts, see the repository [README.md](../README.md).

## Features

- [x] A Typescript-based PEAS plugin system based on Deno runtime
- [x] Measure-aware runtime scoring from plugin performance definitions
- [x] Workspace plugin registry for enabling/disabling plugins
- [x] Dynamic actuator dispatch (native and plugin-process executors)
- [ ] Easily import/remove external PEAS plugins
- [ ] Sensor and actuator runtime priorities
- [ ] Track changes of the environment over time

## Plugin Structure

Each plugin is a small Typescript project which is executed using the Deno runtime.

Rust was *not* chosen for plugins because that would require untrusted executables; with a Typescript-based system, source code is readily available for any executed plugin. Furthermore, Deno runtime provides additional security features, like sandboxing, to make Looper more safe.

For a plugin to be imported and executed correctly, it must follow the structure below.

### Manifest File

The `looper-plugin.json` file should exist in the top-level directory of the plugin project, with the following structure.

#### Top Level

The top level of the manifest contains general plugin information.

| Field | Type | Required? | Description |
|---|---|---|---|
| `name` | Text | Required | Acts as the ID and name (no spaces) |
| `description` | Text | Required | Helps the agent understand the plugin |
| `version` | Text | Required | Version of the plugin |
| `entry` | Text | Required | Entrypoint typescript file location |
| `permissions` | Object | Required | Runtime permissions of the plugin |
| `peas` | Object | Required | Configuration for PEAS components |
| `variables` | List (Object) | Optional | Key-value pairs used by the plugin |

#### Permissions

The `permissions` object provides filesystem and command sandboxing.

| Field | Type | Example | Description |
|---|---|---|---|
| `read` | List (Text) | Required | Allowed directories ("." for all) |
| `run` | List (Text) | Required | Allowed shell commands ("." for all) |

#### PEAS

The `peas` object contains information related to PEAS components that are supplied by the plugin.

| Field | Type | Required? | Description |
|---|---|---|---|
| `performance` | List (Object) | Required | A list of performance measures |
| `actuator_executor` | Text | Optional | Default executor for all actuators (`plugin_process` or `native_filesystem`) |
| `environment` | Object | Optional | A list of environment descriptions |
| `actuators` | List (Object) | Optional | A list of actuators |
| `sensors` | List (Object) | Optional | A list of sensors |

#### Performance

| Field | Type | Required? | Description |
|---|---|---|---|
| `name` | Text | Required | A display name for the measure |
| `description` | Text | Required | Tell the critic about the measure |
| `weight` | Number | Optional | Top-level weight for scoring (defaults to `1.0`) |
| `evaluation_mode` | Text | Optional | Strategy hint (e.g. `strict`, `balanced`) |
| `success_criteria` | List (Text) | Optional | Criteria used by runtime feedback summaries |
| `rewards` | List (Object) | Optional | How to reward good behavior |
| `punishments` | List (Object) | Optional | How to punish bad behavior |

*Note: At least one reward or punishment must be provided.*

##### Rewards / Punishments

| Field | Type | Required? | Description |
|---|---|---|---|
| `name` | Text | Required | A display name for the reward/punishment |
| `when` | Text | Required | When to trigger the reward/punishment |
| `weight` | Number | Required | Weight (0.0-1.0) of the reward/punishment |

#### Environment

| Field | Type | Required? | Description |
|---|---|---|---|
| `name` | Text | Required | A display name for the environment |
| `description` | Text | Required | Tell the agent about the environment |
| `rules` | List (Text) | Optional | Strict rules about the environment |
| `conventions` | List (Text) | Optional | Conventions of the environment |

#### Actuators

| Field | Type | Required? | Description |
|---|---|---|---|
| `name` | Text | Required | Acts as the ID and name (must be in plugin) |
| `description` | Text | Required | Tell the agent about the actuator |
| `executor` | Text | Optional | Per-actuator override (`plugin_process` or `native_filesystem`) |

#### Sensors

| Field | Type | Required? | Description |
|---|---|---|---|
| `name` | Text | Required | Acts as the ID and name (must be in plugin) |
| `description` | Text | Required | Tell the agent about the sensor |

## Workspace Plugin Registry

Runtime plugin activation can be controlled by `./.looper/plugin-registry.json` in each workspace.

```json
{
  "plugins": [
    { "name": "filesystem-read", "enabled": true, "source": "builtin", "version": "0.1.0" },
    { "name": "cybersecurity-pack", "enabled": false, "source": "git+https://...", "version": "1.2.3" }
  ]
}
```

When present, `enabled: false` removes a plugin from active planning/execution for that workspace only.

## Dynamic Actuator Execution

Actuators are dispatched by executor type instead of hardcoded actuator names:

- `native_filesystem`: handled by Looper's Rust runtime.
- `plugin_process`: handled by invoking the plugin entrypoint in Deno.

For `plugin_process`, Looper sends this input payload to plugin stdin:

```json
{
  "kind": "actuator_execute",
  "actuator": "your_actuator",
  "args": { "...": "..." },
  "workspace_dir": "/path/to/workspace"
}
```

And expects this JSON response on stdout:

```json
{
  "status": "completed",
  "details": "optional detail text",
  "sensor_output": "optional sensor summary for the model"
}
```

## Reference External Plugin

A reference external plugin is included at:

`looper-agent/external-plugins/reference-inspector`

You can install it in a workspace from chat with:

`/plugin add looper-agent/external-plugins/reference-inspector`

## Starter Pack Templates

Looper includes an external plugin starter pack for domain-specific workflows:

- `looper-agent/external-plugins/blogging-starter`

You can view bundled starter packs from terminal chat with:

- `/plugin catalog`

Install it with:

- `/plugin add looper-agent/external-plugins/blogging-starter`

## Guidance Priority

- Active plugin performance measures are primary runtime guidance.
- User instructions are always authoritative.
- `SOUL.md` is optional and treated as a secondary overlay, not the default control surface.
