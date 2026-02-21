# Looper

An experimental general purpose agent designed to act continuously and autonomously, developed on top of the [Fiddlesticks](https://github.com/philo-groves/fiddlesticks) harness framework.

## Projects

- `looper-agent`: Core agent runtime functionality
- `looper-terminal`: TUI for communicating with the agent
- `looper-web`: Next.js-based web application

## Key Terms

These definitions will help you understand the language used in this repository:

- **Percept**: A single unit of perception
- **Sensor**: A receiver or detector of percepts
- **Action**: A single unit of doing something
- **Actuator**: An executor for performing actions

*Note: These terms are not new; they are common in the world of robotics.*

## Sensory Loop

Continuous execution is a primary goal of the Looper agent. As the name suggests, this is achieved through a sensory loop and a multi-model architecture. For the best cost-benefit, the agent will delegate its inference to a different model depending on its execution state. Local models are recommended for small tasks, and frontier models are recommended for complex tasks.

By default, the loop does not run at a fixed rate; after an iteration ends, the next will begin immediately. However, global configurations exist for backoff timing.

![Architecture flow](https://i.imgur.com/xX0vLVT.png)

### 1. Surprise Detection

At the beginning of the loop iteration, the local model will list the percepts that have been received through sensors since the last iteration. The local model will read this list, in addition to (up to) 10 previous iterations' lists, to find any percepts that seem surprising in the newest iteration.

If the local model determines there are no surprising new percepts, the loop ends early and there is no requirement for the frontier model at all.

*Note: After the percept list creation, any percepts received mid-iteration are not processed until the next iteration.*

### 2. Reasoning & Planning

At this stage, the frontier model will investigate the surprising new percepts through an extended semantic search of the percept history and other read-only tools. The read-only tools include grep for searching file contents, glob for searching directories, and web search for fetching up-to-date information. Using information gathered by the frontier model, it will determine whether or not to perform actions, which actions to perform, and in what order.

If the frontier model determines no actions are required, the iteration ends early.

### 3. Perform Actions

In this final stage, actuators are used to perform planned actions from the reasoning step. In most cases, these are tools that are executed through tool calls. The tool calls may be internal tools `shell` commands, playwright browser navigation, or MCP server interactions. 

After all actions are performed, the loop iteration is complete.

## Sensors

Sensors are the primary mechanism for Looper to gather information. They allow for scalable input to the agent by classifying inputs of different types and standardizing their connections. Individual sensors can be added, removed, enabled, and disabled as needed.

Each sensor acts as a windowed queue. At the beginning of each sensory loop, the window is moved to the latest, where the queue of percepts are sensed and marked as read. During the execution of the sensory loop, which may take several minutes for complex tasks, more percepts will accumulate in the queue.

### Internal Sensors

- **Chat Sensor**: Receiver of looper app (e.g. looper-terminal) chat messages in the form of percepts

### Add a Sensor

New sensors can be added to the agent for extending its functionality. Setting up a new sensor is relatively simple, you just need:

- Name of the sensor
- Policy of the sensor
- Description of its percepts (helps the agent understand)

#### Add a Sensor through Terminal

Run the following command to be guided step-by-step through the creation process:

`looper sensor add`

#### Add a Sensor through Web Interface

In a browser, navigate to: http://127.0.0.1:10001/sensors/add

Complete the form on the page to add a sensor.

#### Add a Sensor through REST API

Send a POST request to: http://127.0.0.1:10001/api/sensors

Body Format:
```
{
    "name": "",
    "description": ""
}
```

## Actuators

Actuators are the primary mechanism for Looper to perform actions. They allow for scalable output from the agent and the ability to interact with the world. There are several types of actuators: internal tools, MCP servers, and agentic workflows. To get started, you will need:

- Name of the actuator
- Policy of the actuator
- Description of its actions (helps the agent understand)
- Type of the actuator
- Any details required for that actuator type

### Internal Actuators

- **Chat Actuator**: Responder of looper app (e.g. looper-web) chat messages in the form of actions, using RPC
- **Grep Actuator**: Searches the content of text-based files
- **Glob Actuator**: Searches directories for files
- **Shell Actuator**: Performs command line operations
- **Web Search Actuator**: Searches the internet for up-to-date information

### Add an Actuator

In addition to the internal actuators, two other actuator types can be added: MCP servers and agentic workflows.

#### Add an Actuator through Terminal

Run the following command to be guided step-by-step through the creation process:

`looper actuator add`

#### Add an Actuator through Web Interface

In a browser, navigate to: http://127.0.0.1:10001/actuators/add

Complete the form on the page to add an actuator.


#### Add an Actuator through REST API

Send a POST request to: http://127.0.0.1:10001/api/actuators

Body Format:
```
{
    "name": "",
    "description": "",
    "type": "",
    "details": {},
    "policy": {}
}
```

*Note: Valid types are "mcp" and "workflow"*

The `policy` object is optional, but safety-critical. Each of its fields are optional as well:

```
{
    "allowlist": [],
    "denylist": [],
    "rate_limit": {},
    "require_hitl": false,
    "sandboxed": false
}
```

*Note: The `allowlist` and `denylist` are referencing tools for MCP servers, and shell commands for agentic workflows. Both `allowlist` and `denylist` cannot be used together*

The `rate_limit` object has the following fields (example values included). Both fields are required:

```
{
    "max": 100,
    "per": "hour"
}
```

*Note: Allowed values for the `per` field are "minute", "hour", "day", "week", and "month". The `max` field must be greater than 0.*

The `require_hitl` field will require a human-in-the-loop before the action can be completed. This is recommended for risky commands and tools. Approval can be conducted through the terminal, web interface, and REST API.

The `sandboxed` field only relates to agentic workflows, and will ensure its shell commands are executed in a locked down environment with less permissions.

### MCP Servers

The following `details` should be provided when adding a MCP server as an actuator:

```
{
    "name": "",
    "type": "",
    "url": ""
}
```

*Note: Valid types are "http" and "stdio"*. When using the "stdio" type, the url value should be a file path to the server executable (e.g. server.py)

### Agentic Workflows

The following details should be provided when adding an agentic workflow as an actuator; example values are included for clarity:

```
{
    "name": "Bundle Validation & Deployment",
    "cells": [
        "You should validate the Databricks bundle is healthy.",
        "%shell git pull origin main",
        "%shell databricks bundle validate -t dev",
        "If the validation fails, do not continue",
        "If the validation is successful, deploy the bundle.",
        "%shell databricks bundle deploy -t dev"
    ]
}
```

*Note: `%shell` cells will deterministically route through the internal `shell` tool. Safety policies of the internal `shell` tool are enforced through agentic workflows as well.*

### Actuator Executor

Instead of allowing the frontier model to perform actions directly, its tool execution decisions are treated as recommendations to the actuator executor. The executor will compare the recommendation to the safety policies of the actuator, then determine whether or not to perform the action.

If the action is found to violate rate limits, allowlists, or other safety features, it will be deterministically denied. 

### Observability

The following metrics are available through the terminal, web interface, and REST API to help track the agent health and activity:

- Execution counts per iteration phase
- Local and frontier model token usage
- False positive surprises (total and percent)
- Failed tool executions (total and percent)
- Loops Per Minute (LPM)
