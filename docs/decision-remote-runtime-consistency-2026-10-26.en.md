# Decision Record: Remote Runtime Consistency — First Written as Two Layers, Trimmed After Review (2026-10-26)

[中文](decision-remote-runtime-consistency-2026-10-26.md)

This records a discussion that started with "sorting out the remote connection logic": a two-layer design was settled first, review found it too heavy, and it was finally trimmed down to filling in consistency. Design: [remote runtime consistency](design-remote-runtime-consistency-2026-10-26.en.md); issue [#695](https://github.com/arcships/rutis/issues/695).

## 1. Starting point

Going through the remote connection code in 0.17.1, we found that the same remote language runtime behaves differently depending on how it is connected: one connection (a link row + `runtime`) and many connections (an access point) differ in how the configuration is written, in the error for a missing plugin, and in output.

## 2. How the discussion went

1. **How a runtime is connected should not change how plugins are used.** The first explanation used internal terms and several tables and did not make the point clear; it became clear only after switching to one scenario (one plugin on a GPU machine, two ways to connect it, different behavior when used). Conclusion: the connection method should be decoupled from plugin lifecycle and from how configuration takes effect.
2. **The host must be able to build its own connections**: relays, external authentication, a different protocol. The session protocol does not change (it is the shared contract of all language SDKs).
3. **Connection configuration should not be exposed to the person writing the configuration**: the configuration names only the runtime and the plugins.
4. **Settled on two layers**: the remote plugin layer (discovery, binding, lifecycle, handshake) and connection plugins (dial or listen, authentication, reconnection, transport), with a new "connection registry" handover interface between them; link and access point rewritten as built-in connection plugins; authentication opened up; a relay example and contract tests for connection plugins; split into 4 issues.
5. **Review: too heavy.** A self-review and an independent review reached the same conclusion; the key points were verified in the code:
   - The code is already layered. A link only produces `RuntimeSession#<name>`, and an access point hands each connection to an `Accept` callback passed in from outside; neither knows about the loader. The earlier judgment that "the connection side is doing plugin management" went too far.
   - The differences come from two separate code paths for one connection and many connections, not from the connection direction: one connection can already either dial or listen.
   - Building your own connection is possible today: implement `Transport`. The only gap is that rutis-host hard-codes WebSocket for access points.
   - The new handover interface, opened-up authentication, the relay example, and the contract tests have no concrete user.
   - Making authentication awaitable is not a local change: the credential check runs in the WebSocket library's synchronous callback and admission runs in the synchronous `recv`, so the listener routing would have to be rewritten.
   - The two-layer design leaves revocation in the connection plugin and moves admission to the upper layer, so a revoked id would be found only after the row has started loading, breaking the guarantee of #210.
   - Automatic conversion of old configuration is unnecessary: the old form already works, and the precedent for removing a form is to report an error that gives the new form; nothing has ever been converted.
6. **Trimmed**: keep the existing layering and the two handover points, and only fill in consistency: `runtimes.remote` written as a single complete entry, `instances` decides one connection or many connections, optional `transport`, shared discovery and checks, a consistent error for a missing plugin, and output for many connections.

## 3. Decision

- No new handover interface, no rewrite of link or access point; the runtime side, the session protocol, and the Rust API stay unchanged.
- The three fields `dial`, `listen`, and `instances` of `runtimes.remote` are independent; rutis-host generates the existing link and runtime access internally; the old link-row form keeps working and is not converted.
- Optional `transport`, used by both one connection and many connections; building your own connection = implementing `Transport`.
- One connection and many connections share discovery and checks; a missing plugin is uniformly `Unavailable`; connections in many-connections mode produce output.
- Scheduled for 0.19: the change affects remote behavior and must be done before the 0.20 formal coverage. Implementation is merged into #696 (loader) and #698 (rutis-host); the original #697 is folded into #696, and #699 will not be done.
- Two independent problems found during review are filed separately: #701 re-resolve all rows after reconnection (0.17.2), #702 startup error for token conflicts on the same listener (0.18).

## 4. Approaches not adopted

| Approach | Reason not adopted |
| --- | --- |
| Two-layer design: a new "connection registry" handover interface, link and access point rewritten as connection plugins | The two existing handover points already serve this purpose; it rewrites working code; splitting revocation and admission across two layers would break #210 |
| Opened-up authentication: custom credentials, awaitable checks | No user; requires rewriting listener routing |
| Relay example, contract tests for connection plugins | No user, and no connection plugin authors |
| Automatic conversion of old configuration at startup | The old form already works; the precedent is to report an error that gives the new form |
| Connection plugins written as a row in the configuration | Exposes connection details to the person writing the configuration |
| One service per connection, named by connection id | The id is self-reported by the peer and would take up the application's service keys |
| Allow replacing the session protocol | It is the shared contract of all language SDKs; to interoperate with another protocol, write a translation plugin at the boundary |
| Many-connections mode also resolves bare names by `language` (such as npm names) | Rows in many-connections mode are always written `<runtime>:<plugin>`, which is explicit; bare names are ambiguous across multiple runtimes |

## 5. Lessons

- Before judging "where the coupling is", look in the code for existing handover points first. This time the first version took a callback passed in from outside as a direct call, and derived a whole new layer of design from that.
- For every new capability, first ask whether it has a concrete user (design philosophy §7). "The host might want a relay" was an assumption; the existing `Transport` already covers it.
- After a design is finalized, ask a reviewer who was not in the discussion to look at it, specifically asking "is this too heavy?".
