// The test plugins in one binary: weather and conformance-events.
import Rutis
import RutisFixturePlugins

Rutis.serve(weatherPlugin, conformanceEventsPlugin)
