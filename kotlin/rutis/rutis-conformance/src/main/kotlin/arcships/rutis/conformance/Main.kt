// The test plugins in one runtime, served by the desktop test entry
// (Rutis.serve): `java -jar rutis-conformance-all.jar <channel> <project>`.
package arcships.rutis.conformance

import arcships.rutis.Rutis
import arcships.rutis.conformance.plugins.conformanceEvents
import arcships.rutis.conformance.plugins.weather
import arcships.rutis.serve

fun main(args: Array<String>) {
    Rutis.serve(args, weather, conformanceEvents)
}
