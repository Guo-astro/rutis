package arcships.rutis.sample.plugins

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import android.os.Build
import arcships.rutis.Rutis
import arcships.rutis.RutisService
import arcships.rutis.service
import kotlinx.serialization.Serializable

/** What a device is. rutis does not model devices: this plugin says it, for whoever injects `device`. */
@Serializable
data class DeviceInfo(
    val manufacturer: String,
    val model: String,
    /** The Android version, `14`. */
    val release: String,
    /** The API level, `34`. */
    val sdk: Int,
)

@Serializable
data class Battery(
    /** Percent, 0–100. */
    val level: Int,
    val charging: Boolean,
)

/** The `device` service. */
@RutisService
class Device(private val context: Context) {
    fun info(): DeviceInfo = DeviceInfo(Build.MANUFACTURER, Build.MODEL, Build.VERSION.RELEASE, Build.VERSION.SDK_INT)

    fun battery(): Battery {
        val manager = context.getSystemService(BatteryManager::class.java)
        // The sticky battery broadcast: read, not listened to.
        val status = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
            ?.getIntExtra(BatteryManager.EXTRA_STATUS, -1) ?: -1
        return Battery(
            level = manager.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY),
            charging = status == BatteryManager.BATTERY_STATUS_CHARGING || status == BatteryManager.BATTERY_STATUS_FULL,
        )
    }
}

/** The plugin `device`: what this device is, and its battery. Needs no permission. */
fun device(context: Context) = Rutis.define<Rutis.NoConfig>(
    name = "device",
    // Temporary: an undeclared service of a connection is not yet found by
    // the other rows of its instance in rutis-loader; this goes once that
    // is fixed (declarations are optional).
    provides = mapOf("device" to service<Device>()),
) { ctx, _ ->
    ctx.provide("device", Device(context.applicationContext))
}
