package dev.mellow.client

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager

/**
 * Keep-alive foreground service: holds one ongoing "Mellow — connected"
 * notification so Android 12+ does not freeze the app process (and with it
 * the WebView chat WebSocket) while the screen is off. The WebSocket itself
 * belongs to the WebView; this service only prevents process suspension and
 * shows the unread badge. Start/stop is owned by the Rust side via
 * MellowBridge (dispatched from on_navigation).
 */
class MellowKeepAliveService : Service() {

    private var wakeLock: PowerManager.WakeLock? = null

    override fun onCreate() {
        super.onCreate()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            val channel = NotificationChannel(
                CHANNEL_ID, "Mellow connection", NotificationManager.IMPORTANCE_LOW
            )
            channel.description = "Shown while connected to a Mellow server"
            nm.createNotificationChannel(channel)
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notification = buildNotification(this, badge)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIF_ID, notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING
            )
        } else {
            startForeground(NOTIF_ID, notification)
        }
        if (wakeLock == null) {
            val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
            wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "mellow:keepalive").apply {
                setReferenceCounted(false)
                acquire()
            }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    companion object {
        private const val CHANNEL_ID = "mellow-keepalive"
        private const val NOTIF_ID = 42
        @Volatile
        private var badge: String = ""

        fun updateBadge(ctx: Context, count: String) {
            badge = count
            val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            nm.notify(NOTIF_ID, buildNotification(ctx, count))
        }

        private fun buildNotification(ctx: Context, count: String): Notification {
            val tap = PendingIntent.getActivity(
                ctx, 0,
                Intent(ctx, MainActivity::class.java),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
            )
            val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Notification.Builder(ctx, CHANNEL_ID)
            } else {
                @Suppress("DEPRECATION")
                Notification.Builder(ctx)
            }
            builder
                .setContentTitle("Mellow")
                .setContentText(
                    if (count.isEmpty()) "Connected · tap to open"
                    else "$count unread · tap to open"
                )
                .setSmallIcon(R.drawable.ic_stat_mellow)
                .setContentIntent(tap)
                .setOngoing(true)
                .setCategory(Notification.CATEGORY_MESSAGE)
            count.trimEnd('+').toIntOrNull()?.let { builder.setNumber(it) }
            return builder.build()
        }
    }
}
