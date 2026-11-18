// The session-end fixtures in one binary: `swift_lease`, the plugin of
// crates/rutis-host/tests/lease_e2e.rs (a listening runtime whose lease a
// newer controller takes over), and `host_gone`, the plugin of
// crates/rutis-bridge/tests/host_gone.rs (a local runtime whose host goes
// away).
import Rutis
import RutisFixturePlugins

Rutis.serve(swiftLease, hostGone)
