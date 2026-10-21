# Software Starts Adapting to the World on Its Own

[中文](software-that-adapts.md)

> rutis: a container for software that grows

---

For a long time, the relationship between software and its environment was fixed. Software was written, shipped, and then ran in a world it could not change. When the world changed — an API was upgraded, a new system appeared, an old service went away — the software waited for a person to catch up.

That is changing.

Models give software, for the first time, the ability to change itself. Writing some integration code, trying it, finding it wrong and fixing it used to be work you scheduled into a sprint; now it may take a few minutes. Once changing code becomes cheap, a new question comes up:

**How does software keep reaching its environment, adapting to it, and experimenting and exploring in it?**

This is the question rutis sets out to answer.

---

## 1. When changing code is no longer expensive

We used to design software assuming change was expensive. So we thought requirements through first, settled interfaces, pinned dependencies, and then touched things as little as possible.

When change is cheap, that assumption no longer holds. For software that iterates on itself, the everyday activity is not "running" but "trying":

- today it needs to reach an internal calendar system, so it writes a first version;
- tomorrow it finds that system has a better API, and swaps in a new version;
- the day after, it wants a model on a GPU machine, and moves part of its work there;
- a while later, the part used most needs to be steadier and faster, and gets rewritten in another language.

Each of these is a small change. But small changes like these may happen dozens of times a week, in a system that is running.

So the hard part shifts from "how to write it" to "how to keep swapping it": when you swap, are the old connections, timers and listeners left behind? Does what depends on it break along with it? If the swap goes wrong, can you go back? When it moves to another machine or another language, does the code that uses it have to change too?

These questions decide whether a piece of software can actually grow.

## 2. Wait for others to adapt to you, or adapt to them yourself

Today, the mainstream way for software to gain new capabilities is to wait for others to adapt to it.

Whoever maintains the environment provides APIs, SDKs, open platforms, and lately MCP servers, and the software calls them. That is a fine path, but it rests on one premise: **what the software can do depends on what someone outside has prepared for it.** The connection stays outside the software. It belongs to the environment, not to the software.

If software can already write code, that premise is no longer necessary. It can turn things around:

**Spend its own time embracing and adapting to others.**

Whatever it needs to reach, it writes a plugin to connect to it — in whichever language fits best, possibly written by an agent — then tries it, changes it, replaces it. From then on, the connection is part of the software. It has its own lifecycle, it can be observed, and it can be taken apart cleanly.

One step further, software can **eventually replace other people's interfaces and systems**. That sounds like a slogan, but in rutis it comes down to a plain mechanism:

> Whoever uses a service depends only on its **name**, never on its implementation.

Say a scheduling feature depends on a service named `calendar`.

1. At first, `calendar` is provided by a plugin that only wraps the company's existing calendar system;
2. after a while, real use has worked out which methods `calendar` should have and what they return;
3. later, it is replaced by an implementation of its own, still called `calendar`.

Across all three stages, the scheduling feature does not change a single line, and the host program does not restart. **"Adapting" and "replacing" are the earlier and later stages of the same thing**, with no extra migration project in between.

The two approaches can coexist; an MCP client can simply be one plugin inside rutis. But the boundary of what software can do should not be decided by its environment.

## 3. The foundation: swap one piece, while it runs

For software to iterate this way, the first step is to split it into pieces that can be replaced individually at runtime. Those pieces are plugins.

Plugin systems are not new, but "frequently replacing pieces while running" asks a lot of them. The rutis core does only a few things for this, and each one is written down as a promise and pinned by tests:

- **Nothing starts before what it depends on is there.** A plugin declares which services it needs; it starts only when they are all ready.
- **Everything registered is cleaned up exactly once when it stops.** Services, listeners, background tasks and child plugins are released in reverse order of registration — no more, no less.
- **A failed start rolls back.** What was already registered is withdrawn, leaving no half-built state behind.
- **When one piece is swapped, only what uses it restarts.** When the provider of a service changes, the plugins depending on it stop on their own and start again once the new provider is ready. Everything else is untouched.

With these in place, replacing something once is as safe as replacing it a thousand times. Software can then slice its iteration very finely: no need to release the whole thing, just swap one piece; no need to pretend in a test environment, it can be verified in the system that is actually running, and swapped back if it doesn't work.

There is a kind of "hot reload" that replaces machine code while running. rutis does something else: **it replaces running parts**. This idea goes back to Erlang/OTP and Java's OSGi. Rust's ownership model gives "unloading cleanly" a more reliable foundation — in garbage-collected runtimes, a single forgotten reference is enough to keep an old module loaded forever.

But the plugin mechanism is only the foundation. What rutis really wants is to let this kind of iteration **spread outward**.

## 4. Letting iteration spread: across processes, languages and devices

If iteration can only happen inside one process and one language, the environment software can reach is locked inside that ecosystem. To use Python's model ecosystem, you have to pull it in; to use a device on another machine, you write a separate remote-call layer; and every boundary you cross means solving "startup, dependencies, cleanup" all over again.

rutis makes that boundary disappear for plugins: **a plugin does not need to know which process provides a service it depends on, what language it is written in, or which machine it runs on.** It knows only the name. Where the service comes from, what happens when it drops, how to follow when it changes — rutis handles that.

The three examples below each cross one kind of boundary.

### Example 1: an agent whose shell runs on another machine

Suppose rutis itself is an agent: model calls, conversation and decisions all happen on this machine. It needs to run commands, but not on its own machine — perhaps in an isolated sandbox, perhaps on a GPU machine with CUDA installed.

The shell can be a plugin, running in a Python runtime on that machine:

```python
import asyncio
from rutis import define_plugin


class Shell:
    async def run(self, command):
        proc = await asyncio.create_subprocess_shell(
            command, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT
        )
        out, _ = await proc.communicate()
        return out.decode()


def apply(ctx, config):
    ctx.provide("shell", Shell())


plugin = define_plugin(apply, provides={"shell": Shell})
```

That machine runs only a listening Python runtime. Which plugins to load, with what configuration, and when to start and stop them, are all decided by the rutis where the agent lives:

```json
{
  "id": "agent",
  "runtimes": { "remote": [{ "name": "sandbox", "language": "python" }] },
  "rows": [
    { "id": "sandbox", "name": "rutis-bridge/peer",
      "config": { "peer": "sandbox", "dial": "wss://sandbox.example.com:7443/rutis", "runtime": "sandbox" } },
    { "id": "shell", "name": "sandbox:shell_tool" }
  ]
}
```

The agent side depends only on the name `shell`. So:

- **To switch to another execution environment**, change one address in the configuration;
- **When that machine goes offline**, the parts that depend on `shell` stop and wait instead of crashing; once the connection is back, they load again automatically;
- **The execution environment can iterate too**: the shell plugin starts in Python and may later be replaced with a Go version, without touching the agent's code.

The agent's "hand" now reaches another machine, and using it feels no different from using a hand on this one.

### Example 2: a server whose parts are spread across processes and machines

Now a more common case: a backend service.

Its main logic is in Rust, but each thing it needs has its own best place:

```
server main logic (Rust)
  ├─ database client                  same process
  ├─ a third-party API integration    Python process (using the official SDK)
  ├─ inference service                runtime on a GPU machine
  └─ adapter for an old internal system   to be replaced by an implementation of its own
```

Each piece is a plugin that provides services by name, and the main logic uses them by name. Where they run and what language they are written in is written in configuration, not in code.

That brings a few things:

- **Failures have boundaries.** If the Python process crashes, the services it provided are withdrawn and the parts that depend on them stop and wait; everything else in the same host keeps running. rutis can also restart that runtime after a crash, with backoff.
- **Changing configuration doesn't take the service down.** Rereading the configuration while running is all or nothing: if the new configuration has a problem, the old one keeps running and nothing changes.
- **"Adapt first, replace later" happens right here.** The old internal system is first wrapped by an adapter plugin; once the interface has settled under real traffic, it is replaced by an implementation of its own. The main logic never notices.

### Example 3: several rutis hosts networked together, sharing capabilities

The last boundary is the device. Every rutis host can be a node in a network.

```
 office node ──exports calendar──▶  cloud node (running the agent)  ◀──exports device── home node
             ◀──── shared services · plugins run on the other side · forwarded events ────▶
```

Nodes are connected by an encrypted link. Over that link they can:

- **share services**: export a service of this node to the other side, or import the other side's service as one's own;
- **run plugins on the other side**: the plugin runs on the other machine and uses that machine's services, while its configuration and start/stop are decided on this side;
- **forward events**: what happens on one side can be received on the other.

A broken link reconnects automatically; while the other side is offline, the parts that depend on it wait, and recover when it returns.

This is the most direct picture of iteration spreading outward: **each node contributes the capabilities of the environment it sits in** — the office node can reach internal systems, the home node can reach devices at home, the cloud node has compute. Once one node learns to reach some system, every piece of software on the network gains that capability.

Further on, phone and desktop apps can join the same way: an app turns capabilities like contacts, location and notifications into plugins, connects to rutis, and becomes part of the network. This part is still being designed, but the direction is the same.

## 5. What stays fixed goes to Rust; what changes goes to the ecosystems

All of this rests on one basic division of labour in rutis:

**Give what is fixed and hard to change to a low-level language; give what changes and needs an ecosystem to other languages.**

- **The fixed part** is the lifecycle itself: what starts first, what happens when a dependency disappears, how cleanup works, and what cannot be done once you cross a process boundary. rutis implements these rules once, in Rust. The core depends on only a handful of libraries such as tokio, and every guarantee is registered and mapped to specific tests.
- **The changing part** is connection and adaptation: Python has the AI and data ecosystem, Node has npm and existing Cordis plugins, Go has a wealth of infrastructure libraries, and another machine has that machine's processes and devices. On the side of those languages there is only a small SDK, not a second framework.

This division also has a meaning over time. A capability usually starts in Python or TypeScript, tried quickly; through real use its shape slowly settles; and the parts that have stood the test of time are rewritten in Rust for stability and performance. An early design note in the project puts it well:

> TS is the lab; Rust takes the graduates.

What deserves to "graduate"? Things whose semantics have converged, that are mechanism rather than wording, that are performance-sensitive, or that sit on a trust boundary. For an agent: **the machinery that assembles prompts will be hardened; the content of the prompts never will.**

Because users of a service know only its name, this path from experiment to hardening can be walked one step at a time, with no day when everything stops for a big migration. The more runtimes software can plug into, the more ecosystems its iteration can reach.

## 6. A new container for growth

Placing rutis next to an operating system makes clearer what it is.

| | A traditional operating system | rutis |
| --- | --- | --- |
| Its assumption | Programs are already written and run inside it | Programs keep growing, changing as they run |
| What it manages | CPU, memory, processes | Connections and capabilities: who provides what, who depends on whom |
| Its boundary | One machine | Across processes, languages and devices |
| How programs gain capabilities | Install software, install drivers | Write a plugin to connect, try it, replace it |
| The question it cares most about | Isolation and scheduling | After one piece is swapped, is the whole system still correct and clean? |

rutis does not replace the operating system; it runs on top of it. The operating system lets programs run; rutis wants to let them **grow**. Seen from rutis, plugins are like processes, services are like system calls, and language runtimes and remote nodes are like drivers — all managed by the same lifecycle, none with special privileges.

## 7. The costs, and what isn't done yet

This design has costs, and rutis writes them down too:

- **Processes and latency.** Each language needs at least one process; a synchronous call across languages takes about 30 microseconds, far slower than a call within a process. Hot paths belong in the same runtime, or hardened into Rust.
- **Weaker semantics across boundaries.** Past a process boundary, events can only be notifications, and some patterns that hold inside one process no longer do. rutis's rule is to write down what cannot be done as explicit rules instead of quietly degrading; for example, a synchronous call that would deadlock across processes returns an error instead of hanging.
- **Failure scope.** Plugins in the same runtime share a process; if one crashes the process, the others in it stop too. For isolation, run another instance of the runtime.
- **Permissions are still coarse.** Today, trust between nodes is at the level of "a trusted peer". The more devices and runtimes join, the more finer-grained authorization is needed: which node, which plugin may provide or use which services. This is what comes next.
- **Still 0.x.** The API is still evolving; incompatible changes come with migration guides.

---

## Closing

For the first time, software has the chance to understand and embrace the world it lives in on its own: not waiting for others to prepare interfaces for it, but walking over, connecting, trying, improving — and, when needed, taking over.

That needs a container that keeps it from breaking amid constant change, a container not trapped by processes, languages or devices. That is what rutis wants to be.

rutis's lifecycle model comes from Shigma's [Cordis](https://github.com/shigma/cordis). rutis implements it again in Rust and carries it to more languages, processes and machines.

**Try it:**

```bash
npx @arcships/rutis-host new weather --lang node
cd weather && npm install
npx rutis-host dev        # run the plugin, reload it when files change
```

Python, Bun and Go work as starting points too.

- GitHub: https://github.com/arcships/rutis
- Design philosophy: https://github.com/arcships/rutis/blob/main/docs/design-philosophy.en.md
