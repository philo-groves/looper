# Looper

An experimental continuous AI agent based on the PEAS (performance measure, environment, actuators, sensors) architecture from robotics, and its harness is based on the [Fiddlesticks](https://github.com/philo-groves/fiddlesticks) framework.

## Crates

- **looper-agent (bin)**: An instance of Looper running as a background process.
- **looper-terminal (bin)**: A terminal interface (TUI) for communicating with an agent.
- **looper-settings (lib)**: A helper library for managing agent settings, used by `looper-agent`.
- **looper-peas (lib)**: A helper library for managing PEAS plugins, used by `looper-agent`.

![Agent Flow](https://i.imgur.com/nmyPI8u.png)

## Task Environments / PEAS

The following terms, commonly used in robotics, are also used by this agent. In concert, active PEAS components compose the *task environment*.

To avoid terminology confusion between task environments and the environment component of PEAS, the "PEAS" acronym is generally used synonymously with "task environment".

### PEAS Components

- **[P]erformance Measure**: Defines the criteria for success and how to evaluate the agent's behavior (e.g., accuracy, speed, safety, profit).
- **[E]nvironment**: The external surroundings or context in which the agent operates (e.g., a roadmap for a self-driving car, a virtual chat room for a bot).
- **[A]ctuators**: The mechanisms or tools the agent uses to perform actions and affect the environment (e.g., robotic arms, screens, steering wheels).
- **[S]ensors**: The devices used by the agent to perceive or gather data from its environment (e.g., cameras, microphones, GPS, LIDAR).

#### Examples

*Figure 2.5 from [Artificial Intelligence: A Modern Approach (4th Edition)](https://www.pearson.com/en-us/pearsonplus/p/9780137505135)*

| **Agent Type** | Performance Measure? | Environment? | Actuators? | Sensors? |
|---|---|---|---|---|
| **Medical Diagnosis System** | Healthy patient, reduced costs | Patient, hospital, staff | Display of questions, tests, diagnoses, treatments | Touchscreen/voice entry of symptoms and findings |
| **Satellite Image Analysis Program** | Correct categorization of objects, terrain |Orbiting satellite, downlink, weather | Display of scene categorization | High-resolution digital camera |
| **Part-Picking Robot** | Percentage of parts in correct bins |Conveyer belt with parts; bins | Jointed arm and hand | Camera, tactile and joint angle sensors |
| **Refinery Controller** | Purity, yield, safety |Refinery, raw materials, operators | Valves, pumps, heaters, stirres, displays | Temperature, pressure, flow, chemical sensors |
| **Interactive English Tutor** | Student's score on test |Set of students, testing agency | Display of exercises, feedback, speech | Keyboard entry, voice |

### Task Environment Properties

- **Obervable**: A task environment with sensors can be *fully* or *partially* observable. If an agent has no sensors at all, it is *unobservable*. When a task environment is fully observable, the sensors detect that all relevant aspects to the action are known by the agent at each point in time. Relevance depends on the performance measure. If sensors are inaccurate or noisy, or if there is missing sensor data, a task environment is considered partially observable.
- **Agents**: A task environment can be *single-agent* or *multi-agent*. The most simple are single-agent environments, where an agent acts independently. In multi-agent environments, it is important for an agent to understand its interactions with other agents: they may act competitively in a game, or cooperatively in a company. In some cases, an environment can be partially competitive (e.g. taxi drivers competing for rides).
- **Deterministic**: A task environment can be *deterministic* or *nondeterministic*. It is deterministic if its next state can be completely determined by the current state and the agent's actions. Otherwise, the environment is nondeterministic. Sometimes, the term *stochastic* is used for probability-based nondeterministic task environments.
- **Episodic**: A task environment can be *episodic* or *sequential*. It is episodic if the current state of the agent does not depend on previous states. For example, a part-picking robot does not need to know about previous parts on the conveyer belt in order to inspect the current part. If a task environment requires knowledge of its state history, as with conversational agents, it is considered sequential.
- **Static**: A task environment can be *static* or *dynamic*. It is static if the agent does not need to monitor the environment while it is deliberating; these are the most simple environments. In a dynamic environment, the agent continuously asks itself what it wants to do; if the agent decides to do nothing, that counts as an action. An environment is *semidynamic* if the performance measure changes but the environment is otherwise static. 
- **Discrete**: A task environment can be *discrete* or *continuous*. It is discrete if the possible actions are finite and countable, allowing the agent to enumerate and consider all states. Otherwise, the environment is continuous: percepts, actions, and states exist along a continuum.

#### Examples

*Figure 2.6 from [Artificial Intelligence: A Modern Approach (4th Edition)](https://www.pearson.com/en-us/pearsonplus/p/9780137505135)*

| **Agent Type** | Observable? | Agents? | Deterministic? | Episodic? | Static? | Discrete? |
|---|---|---|---|---|---|---|
| **Crossword Puzzle** | Fully | Single | Deterministic | Sequential | Static | Discrete |
| **Chess with a Clock** | Fully | Multi | Deterministic | Sequential | Semi | Discrete |
| **Poker** | Partially | Multi | Stochastic | Sequential | Static | Discrete |
| **Backgammon** | Fully | Multi | Stochastic | Sequential | Static | Discrete |
| **Taxi Driving** | Partially | Multi | Stochastic | Sequential | Dynamic | Continuous |
| **Medical Diagnosis** | Partially | Single | Stochastic | Sequential | Dynamic | Continuous |
| **Image Analysis** | Fully | Single | Deterministic | Episodic | Semi | Continuous |
| **Part-Picking Robot** | Partially | Single | Stochastic | Episodic | Dynamic | Continuous |
| **Refinery Controller** | Partially | Single | Stochastic | Sequential | Dynamic | Continuous |
| **English Tutor** | Partially | Multi | Stochastic | Sequential | Dynamic | Discrete |