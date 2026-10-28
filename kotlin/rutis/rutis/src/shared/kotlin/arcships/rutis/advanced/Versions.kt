@file:Suppress("DEPRECATION")

package arcships.rutis.advanced

import arcships.rutis.Rutis

/** This SDK's version: the runtime's implementation version (the release train checks it). */
const val VERSION: String = Rutis.VERSION

/** The plugin API this SDK writes plugins against and its runtime supports. */
const val PLUGIN_API: Int = Rutis.PLUGIN_API

/** The name the runtime reports itself by: the Maven coordinates it is published as. */
const val IMPLEMENTATION: String = Rutis.IMPLEMENTATION
