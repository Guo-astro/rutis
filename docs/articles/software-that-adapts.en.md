# Software Starts Adapting to the World on Its Own

[中文](software-that-adapts.md)

The moment a piece of software is finished, it is fixed in the environment of that moment.

Afterwards, the interfaces it relies on get upgraded, new systems appear beside it, old services are retired. Each time the environment changes, the software can only stay where it is and wait for a person to read the documentation, change the code and release it again. For decades this has been close to the fate of software: what it can reach depends on what others have prepared for it, and on whether someone is willing to keep up with the changes on its behalf.

Models have, for the first time, loosened this. Software is beginning to read an unfamiliar interface, write the code to connect to it, run it, see that the result is wrong, and write another version. Work that used to be scheduled into an iteration plan may now take a few minutes.

Once changing code is no longer expensive, a deeper question comes into view: can software keep reaching the environment it lives in, experiment and explore there, and adjust itself again and again to fit it?

rutis was made for this question. What follows traces the growth of one agent: what it ran into at each step, and how rutis responds.

## It wants to read a calendar

At first the agent runs on a single laptop. One day it needs to read the company calendar so it can arrange meetings for people.

The company's calendar system is old. Nobody has made an SDK for it, let alone an MCP server. In the old way of doing things, this is where it would stop: the environment isn't ready, so the software can't do it.

But this agent can write code. It reads the calendar system's interface documentation, writes code to connect to it, and installs that code into itself as a plugin that provides a service named `calendar`. From then on, the part that arranges meetings depends only on that name.

This step carries rutis's most fundamental position: **the one who makes the connection should be the software itself, not the maintainer of the environment.** The mainstream approach today is for the environment to package its capabilities and for software to call them. That is a fine path, but the connection always stays outside the software, and what the software can do is decided by what exists outside. rutis wants to reverse this: whatever the software needs to reach, it writes a plugin to connect to it, in whichever language fits best, possibly written by an agent. Once connected, the connection is part of the software, with its own lifecycle; it can be observed, and it can be taken apart cleanly. The two approaches do not conflict; an MCP client can simply be one plugin inside rutis.

What "depending only on a name" is worth will show later.

## Ten changes a day

The first version of the integration is of course imperfect. The calendar has duplicate meetings, there are time-zone problems, and some calls time out. Within a day the agent changes the plugin more than ten times.

Each change means taking the old plugin out of a running system and putting the new one in. This is where the real trouble begins. Did the old plugin close its connections? Are the timers it registered still there? The meeting logic is using the old `calendar` at the moment of the swap; what happens to it? If the new version fails to start, is the system left half old and half new?

Swap once, and care may be enough to avoid these problems. Swap a few hundred times, and any residue accumulates until the system breaks at some moment nobody can explain.

So the rutis core does only a few things, but each one is written down as a promise and pinned by tests. A plugin declares which services it needs, and starts only when they are all ready. The services, listeners, background tasks and child plugins it registers while running are cleaned up in reverse order of registration when it stops, exactly once. A plugin that fails to start rolls back what it had registered, leaving no half-built state. When a service gets a new provider, the plugins that depend on it stop on their own and start again once the new provider is ready; parts unrelated to it are not affected.

With these guarantees, replacing something a thousand times is as safe as replacing it once. Iteration can therefore be sliced very finely: no need to release the whole thing, only one piece is swapped; no need to simulate in a test environment first, the change can be verified in the system that is actually running, and swapped back if it is wrong.

The idea of replacing parts while running was not invented by rutis; Erlang/OTP and Java's OSGi have walked this road. An important reason rutis implements it in Rust is the matter of unloading cleanly: in a garbage-collected environment, a single forgotten reference is enough to keep an old module in memory forever, while Rust's ownership makes it explicit who owns each resource and when it is released.

## It needs another machine

A few weeks later the agent is doing more for people, and much of it involves running commands: running tests, processing data, calling tools installed only on certain machines. It would rather not do this on the laptop it lives on. A better place is an isolated sandbox, or a GPU machine with CUDA installed.

In most systems this means setting up a separate remote-call layer: define a protocol, handle dropped connections, decide who starts and who stops the process on the far side. In other words, the lifecycle problems already solved within one process have to be solved again once you cross to another machine.

rutis does not treat "remote" as a separate problem. The ability to run commands is written as an ordinary Python plugin that provides a service named `shell`, running in a Python runtime on the sandbox machine. That machine only needs to start the runtime and wait for a connection; which plugins to load, with what configuration, and when to start and stop them are all decided by the rutis the agent runs in. On the agent's side it still depends only on the name `shell`, without knowing, or needing to know, which machine it runs on or what language it is written in.

So switching to another execution environment means changing one address in the configuration. When the sandbox machine drops off, the parts that depend on `shell` stop and wait instead of failing; when the connection is back, the plugin is loaded again and those parts recover with it. Some time later the `shell` plugin is rewritten from Python into Go, and not a line of the agent's code changes.

This is what rutis is really after. The plugin mechanism is only the base; what it wants is for this fine-grained iteration to cross the boundaries of processes, languages and devices and spread outward. A plugin knows only the names of the services it depends on. Which process provides a service, in which language, on which machine, rutis takes care of; how to wait when it is gone and how to follow when it returns, rutis takes care of too.

## It starts serving others

Later still, the agent's abilities are turned into a service, and other systems begin to depend on it. It changes from a program on a desktop into a backend.

Each part of the backend has its own best place. The main logic is written in Rust, and the database client lives in the same process. A third-party platform offers only an official Python SDK, so that part runs in a Python process. Inference sits on a GPU machine. And there is an internal system left over from years ago, which for now can only be caught by an adapter plugin.

Each part is a plugin that provides services by name; where it runs and what language it is in are written in configuration, not in code. If the Python process crashes, the services it provided are withdrawn, the parts depending on them stop and wait, and the rest keeps running; if a restart policy is configured, rutis brings that runtime back up with backoff. When configuration is changed while running, the new configuration is checked in full first; if anything is wrong, the old configuration keeps running and nothing changes.

The adapter for the internal system eventually lives out its life too. It runs under real traffic for several months, and call after call works out which methods its service should have and what they return. So the team writes an implementation of its own, provides a service of the same name, and takes the adapter out. The main logic that depends on it is not changed, and the service does not restart.

This is what depending only on a name is worth. Software first adapts to someone else's interface, learns in use what it actually needs, and can finally replace it with an implementation of its own. Adapting and replacing are the earlier and later halves of the same thing, with no separate migration in between. Future software need not wait for others to adapt to it. It can spend enough time to embrace others, adapt to them, and eventually even replace their interfaces and systems.

## More than one of it

When there is more than one such rutis host, things go a step further.

A machine in the office can reach the internal network, a machine at home is connected to the devices there, and a machine in the cloud has enough compute. Each runs rutis, and they connect to each other over encrypted links. Over a link, a node can export its own services to the other side, or import the other side's services as its own; it can run a plugin on the other machine, letting it use the services there while keeping its configuration and its starting and stopping on its own side; and it can forward events from one side to the other. A broken link reconnects by itself; while the other side is offline, the parts that depend on it wait, and recover when it returns.

In this way every node contributes the capabilities of the environment it sits in. Once one node learns to reach some system, the other nodes in the network can use that capability as well. Software's adaptation to its environment is no longer confined to the one machine it runs on.

Along the same direction, phone and desktop apps can turn capabilities such as contacts, location and notifications into plugins and connect them. This part is still being designed, but it follows the same rules.

## What goes underneath, and what goes to the ecosystems

Looking back, all of this rests on one basic choice rutis makes in its technology stack: give what is fixed and hard to change to a low-level language, and give what changes and needs an ecosystem to other languages.

What does not change is the lifecycle itself: what starts first, what happens when a dependency disappears, how cleanup works, and what cannot be done once a process boundary is crossed. rutis implements these rules once, in Rust. The core depends on only a handful of libraries such as tokio, and every guarantee is registered and mapped to specific tests.

What changes is connection and adaptation. Python has the AI and data ecosystem, Node has npm and existing Cordis plugins, Go has a wealth of infrastructure libraries, and another machine has that machine's processes and devices. On the side of those languages rutis provides only a small SDK, rather than rebuilding a framework in each language; each language gets one process, and plugins in the same language call each other directly within it.

This choice also has a meaning over time. A capability often starts in Python or TypeScript and is tried quickly; in use, its shape slowly settles; and the parts that have stood the test of time are rewritten in Rust for stability and performance. There is a plain test for what deserves to be hardened: semantics that have converged, mechanism rather than wording, performance-sensitive code, things that sit on a trust boundary. For an agent, the mechanism that assembles prompts will be hardened; the content of the prompts never will.

Because users of a service know only its name, this path from experiment to hardening can be walked one stretch at a time. The more runtimes software can plug into, the more ecosystems its iteration can reach.

## A container for software that grows

An operating system solves how programs run: it allocates resources to them, isolates them from each other, and schedules their execution. It assumes a program is already written and runs inside it, occasionally replaced as a whole.

rutis cares about something else: how a program that keeps changing stays correct while it changes. What it manages is not CPU and memory but the relationships between capabilities: who provides what, who depends on whom, who must restart when one part is swapped, who must wait when a connection drops. Its boundary is not one machine but every process, language and device the program reaches.

So rutis does not replace the operating system. It runs on top of it and does what the operating system never set out to do: give software that keeps growing a container in which growing does not make it break.

## What it costs

This design has costs, and rutis writes them down.

Each language brought in adds at least one process; a synchronous call across languages takes about 30 microseconds, far slower than a call within a process, so hot paths belong in the same runtime or should be hardened into Rust. Past a process boundary, some semantics that hold inside one process no longer hold; events, for example, can only be notifications. rutis writes what cannot be done as explicit rules instead of quietly degrading: a synchronous call that would deadlock across processes returns an error instead of hanging. Plugins in the same runtime share a process, so if one crashes it, the others stop too; for isolation, run another instance of the runtime. Trust between nodes currently has only one level, "a trusted peer"; the more devices are connected, the more finer-grained authorization is needed, and that is the next piece of work. rutis is also still at 0.x, and its interfaces are still evolving.

## Closing

For the first time, software has the chance to understand the world it lives in on its own: not waiting for others to prepare interfaces for it, but going over, connecting, trying, improving, and, when needed, taking over.

That needs a place that stays reliable amid constant change, a place not confined by processes, languages or devices. That is what rutis wants to be.

rutis's lifecycle model comes from Shigma's [Cordis](https://github.com/shigma/cordis). rutis implements it again in Rust and carries it to more languages, processes and machines.

To try it, start with one command:

```bash
npx @arcships/rutis-host new weather --lang node
```

Python, Bun and Go work as starting points too. The project is at [github.com/arcships/rutis](https://github.com/arcships/rutis), and the thinking behind its design is in the [design philosophy](https://github.com/arcships/rutis/blob/main/docs/design-philosophy.en.md).
