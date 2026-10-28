package arcships.rutis.advanced

import arcships.rutis.Plugin
import arcships.rutis.Rutis
import arcships.rutis.manifestOf
import arcships.rutis.projectDir

/** The project directory the host gave this runtime (its last argument). */
val Rutis.project: String get() = projectDir

/** What the plugins declare, as `--rutis-manifest` prints it. */
fun Rutis.manifest(vararg plugins: Plugin): String = manifestOf(plugins.toList())
