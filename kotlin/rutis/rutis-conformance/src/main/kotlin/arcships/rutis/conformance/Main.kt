// The test plugins in one runtime, served by the desktop test entry
// (Rutis.serve): `java -jar rutis-conformance-all.jar <channel> <project>`.
package arcships.rutis.conformance

import arcships.rutis.Rutis
import arcships.rutis.conformance.plugins.conformanceEvents
import arcships.rutis.conformance.plugins.ktConsumer
import arcships.rutis.conformance.plugins.ktForecast
import arcships.rutis.conformance.plugins.ktGated
import arcships.rutis.conformance.plugins.ktLocal
import arcships.rutis.conformance.plugins.ktProvider
import arcships.rutis.conformance.plugins.ktRelay
import arcships.rutis.conformance.plugins.marker
import arcships.rutis.conformance.plugins.probe
import arcships.rutis.conformance.plugins.registrant
import arcships.rutis.conformance.plugins.valuesBlocking
import arcships.rutis.conformance.plugins.valuesCrash
import arcships.rutis.conformance.plugins.valuesOrigin
import arcships.rutis.conformance.plugins.weather
import arcships.rutis.serve

fun main(args: Array<String>) {
    Rutis.serve(args, weather, conformanceEvents, ktProvider, ktConsumer, ktLocal, ktGated, ktForecast, ktRelay, probe, registrant, marker, valuesOrigin, valuesCrash, valuesBlocking)
}
