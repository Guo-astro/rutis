# Decision Record: Remote Connection Layering — Decoupling the Remote Plugin Layer from Connection Plugins (2026-10-26)

[中文](decision-remote-connection-layering-2026-10-26.md)

This records a discussion that started with reviewing the remote connection logic and ended with two layers. It preserves the process and the reasoning. The design is in [Remote connection layering](design-remote-connection-layering-2026-10-26.en.md); issue [#695](https://github.com/arcships/rutis/issues/695).

## 1. Starting point

Reviewing the remote connection code in 0.17.1 showed four ways for something external to connect to rutis: a local runtime, a remote runtime that rutis connects to (a link row + `runtime`), a remote runtime that connects to rutis (an access point), and another rutis (node link).

The middle two are the same thing with the connection in opposite directions, yet in rutis they take two code paths. They differ in service visibility (global / only within the group), in the error when the remote side lacks a plugin (the whole row fails / only this connection fails), in output (only the link row / nothing printed), and in how they are configured (in two places without cross-checking / in one place).

## 2. How the discussion went

1. **Connection direction should not change how a plugin is used.** The first explanation used internal terms and several tables and did not make it clear; switching to one scenario (one plugin on a GPU machine, connected in two ways, behaving differently in use) made it clear. Conclusion: the connection method should be decoupled from the plugin's lifecycle and from how configuration takes effect.
2. **"One machine or several" is a real difference, but it should be decided by configuration.** Today it is decided by direction: connecting in is treated as several machines, rutis connecting out is treated as one.
3. **The host owner must be able to build their own connections.** Relays, external authentication services, replacing the protocol entirely. Layer by layer: the transport can already be replaced (`Transport`); authentication can be replaced but only accepts tokens and certificates and cannot wait for an external response; connection management is hard-coded as two kinds, link and access point; the session protocol is not replaceable (it is the shared convention of all language SDKs).
4. **The connection itself is a plugin.** In the code, link and access point are already plugins, but their outputs are not uniform: link provides a service, while the access point calls the loader directly in code, which user plugins cannot do.
5. **Connection configuration should not be exposed.** A proposal to write the connection plugin as a row in the configuration was rejected: people who write configuration care only about which plugins run on which runtime, not about how they connect; connection plugins are mounted by the host.
6. **Settled on two layers.** The remote plugin layer handles the discovery, binding and lifecycle that only remote use needs to maintain; connection plugins focus on solving connection. The only thing handed over between the two layers is a connection.

## 3. Decision

- Two layers: the remote plugin layer (discovery, binding, lifecycle, handshake), one shared copy in rutis; connection plugins (dialing out or listening, authentication, reconnection, transport), replaceable, and there can be several.
- Handover: runtime name, connection id, message channel; handed over on connect, taken back on disconnect. Connection plugins depend on the "connection registry" provided by the remote plugin layer and do not implement the rutis protocol.
- One machine or several is decided by `instances`; replacement on the same id belongs to the remote plugin layer.
- Configuration states only the runtime name, the plugins, and the address of the built-in connection; one `runtimes.remote` entry says it all, and the link row is no longer needed.
- The runtime side is unchanged; old configuration continues to be accepted.
- Scheduled for 0.19 (host owner): building one's own connection is an extension point for the host owner; the change alters remote behavior and must be finished before the 0.20 formal coverage, so the rules do not have to be covered twice.

## 4. Approaches not adopted

| Approach | Reason not adopted |
| --- | --- |
| Fix only the differences between the two connection methods, without layering | The differences would disappear, but the host still could not build its own connections; the connection side would keep doing plugin management, and the next connection method would write another copy |
| Write the connection plugin as a row in the configuration, referenced by the runtime declaration | Exposes connection details to people who write configuration; changing the connection method requires changing configuration |
| Each connection provides a service named by its connection id, and the loader watches for services to appear | The id is self-reported by the other side and would take up the application's service keys; it would also need a new mechanism for watching services appear. Instead, the remote plugin layer provides a registry that connection plugins depend on, an ordinary dependency |
| Connection plugins complete the rutis handshake themselves and hand over a session | Every connection plugin would have to implement protocol version and capability checks, and the rules would diverge; instead only a message channel is handed over, and the handshake is done once in the remote plugin layer |
| Allow replacing the session protocol | It is the convention shared by the Python, Node, Bun, Go, Swift and Kotlin SDKs; to interface with another protocol, write a translation plugin at the boundary |
| Bring local runtimes and node links in at the same time | Local runtimes have their own launch and compatibility protocol, and node link's link already follows the provide-a-service pattern; including them would widen the scope, so each is discussed separately |

## 5. Impact

- Only one set of rules remains for remote runtimes; connection direction no longer affects plugin behavior.
- The host owner gains a narrow extension point: write a connection plugin that hands over a message channel.
- Incompatible: the Rust API removes the access point, `RuntimeAccessPlugin`, `RuntimePlugin::remote`, and so on; `rutis-host` output prefixes and several error behaviors change. Accompanied by "Migrating to 0.19".
- In the old design, the connection part of runtime access is marked as superseded and the rest is kept; see design §11 for the mapping.
