# Looper

An experimental continuous AI agent based on the PEAS (performance measure, environment, actuators, sensors) architecture from robotics, and its harness is based on the [Fiddlesticks](https://github.com/philo-groves/fiddlesticks) framework.

## Crates

- **looper-agent (bin)**: An instance of Looper running as a background process.
- **looper-terminal (bin)**: A terminal interface (TUI) for communicating with an agent.
- **looper-settings (lib)**: A helper library for managing agent settings, used by `looper-agent`.
- **looper-peas (lib)**: A helper library for managing PEAS plugins, used by `looper-agent`.

![Agent Flow](https://i.imgur.com/E8mRLFp.png)

## PEAS Terminology

The following terms, commonly used in robotics, are also used by this agent.

- **[P]erformance Measure**: Defines the criteria for success and how to evaluate the agent's behavior (e.g., accuracy, speed, safety, profit).
- **[E]nvironment**: The external surroundings or context in which the agent operates (e.g., a roadmap for a self-driving car, a virtual chat room for a bot).
- **[A]ctuators**: The mechanisms or tools the agent uses to perform actions and affect the environment (e.g., robotic arms, screens, steering wheels).
- **[S]ensors**: The devices used by the agent to perceive or gather data from its environment (e.g., cameras, microphones, GPS, LIDAR).
