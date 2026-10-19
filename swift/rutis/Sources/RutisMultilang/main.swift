// The plugins of the suites that run several runtimes, in one binary.
import Rutis
import RutisFixturePlugins

Rutis.serve(swiftProvider, swiftConsumer, swiftLocal, swiftGated, swiftRegistrant, swiftRelay, callerPlugin, probePlugin)
