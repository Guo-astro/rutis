package arcships.rutis.sample.plugins

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.os.Build
import arcships.rutis.Rutis
import arcships.rutis.RutisService
import arcships.rutis.service
import java.util.concurrent.atomic.AtomicInteger

/** The `notify` service: notifications on this device, from rutis. */
@RutisService
class Notify(private val context: Context) {
    private val manager = context.getSystemService(NotificationManager::class.java)
    private val next = AtomicInteger(1)

    /** Shows a notification; its id, for [cancel]. */
    fun post(title: String, text: String): Int {
        if (Build.VERSION.SDK_INT >= 33) context.require(Manifest.permission.POST_NOTIFICATIONS)
        if (!manager.areNotificationsEnabled()) throw PermissionDenied(Manifest.permission.POST_NOTIFICATIONS)
        val id = next.getAndIncrement()
        val notification = Notification.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_dialog_info)
            .setContentTitle(title)
            .setContentText(text)
            .setAutoCancel(true)
            .build()
        manager.notify(id, notification)
        return id
    }

    fun cancel(id: Int) {
        manager.cancel(id)
    }

    companion object {
        const val CHANNEL: String = "rutis"
    }
}

/**
 * The plugin `notify`. It loads without the permission to post
 * notifications (Android 13 and later ask for it): a post without it fails,
 * and the caller gets `permission-denied` (design §六, the call-time case).
 */
fun notify(context: Context) = Rutis.define<Rutis.NoConfig>(
    name = "notify",
    // Temporary, as for `contacts`.
    provides = mapOf("notify" to service<Notify>()),
) { ctx, _ ->
    val app = context.applicationContext
    app.getSystemService(NotificationManager::class.java)
        .createNotificationChannel(NotificationChannel(Notify.CHANNEL, "rutis", NotificationManager.IMPORTANCE_DEFAULT))
    ctx.provide("notify", Notify(app))
}
