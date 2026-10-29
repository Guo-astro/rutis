package arcships.rutis.sample.plugins

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.BatteryManager
import android.os.Build
import arcships.rutis.Rutis
import arcships.rutis.RutisService
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

/** The `device` plugin's configuration: the name it provides its service as. */
@Serializable
data class DeviceConfig(val service: String = "device")

/** The plugin `device`: what this device is, and its battery. Needs no permission. */
fun device(context: Context) = Rutis.define<DeviceConfig>(name = "device") { ctx, config ->
    ctx.provide(config.service, Device(context.applicationContext))
}
