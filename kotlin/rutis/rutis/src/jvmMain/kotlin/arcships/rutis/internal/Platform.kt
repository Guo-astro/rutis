package arcships.rutis.internal

/** Whether this is the application's main (UI) thread, where a call must not wait for rutis: none on the desktop JVM. */
internal fun onMainThread(): Boolean = false
