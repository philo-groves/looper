# looper-peas

This library contains the core functionality for Looper agents to load and run PEAS plugins. If you are not familiar with PEAS concepts, see the repository [README.md](../README.md).

## Features

- [ ] A Typescript-based PEAS plugin system based on Deno runtime
- [ ] Easily import/remove external PEAS plugins
- [ ] Custom key-value metadata for PEAS plugins and/or their components
- [ ] Easily activate/deactivate PEAS plugins and/or their components
- [ ] Sensor and actuator runtime priorities
- [ ] Track changes of the environment over time
- [ ] Audit of performance after each task

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
| `environment` | Object | Optional | A list of environment descriptions |
| `actuators` | List (Object) | Optional | A list of actuators |
| `sensors` | List (Object) | Optional | A list of sensors |

#### Performance

| Field | Type | Required? | Description |
|---|---|---|---|
| `name` | Text | Required | A display name for the measure |
| `description` | Text | Required | Tell the critic about the measure |
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

#### Sensors

| Field | Type | Required? | Description |
|---|---|---|---|
| `name` | Text | Required | Acts as the ID and name (must be in plugin) |
| `description` | Text | Required | Tell the agent about the sensor |
