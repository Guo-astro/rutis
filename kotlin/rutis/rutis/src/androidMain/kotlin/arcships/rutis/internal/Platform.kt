package arcships.rutis.internal

import android.os.Looper

/** Whether this is the application's main (UI) thread, where a call must not wait for rutis. */
internal fun onMainThread(): Boolean = Looper.getMainLooper().isCurrentThread
