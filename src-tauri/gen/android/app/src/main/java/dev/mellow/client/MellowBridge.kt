package dev.mellow.client

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings

/**
 * Static entry points called from Rust over JNI (wry main-thread dispatch).
 * The Rust side owns the connected/disconnected lifecycle (navigation hooks
 * in lib.rs); this bridge only translates it into service start/stop and
 * notification badge updates.
 */
class MellowBridge private constructor() {
    companion object {
        private lateinit var appContext: Context
        private var batteryPromptShown = false

        @JvmStatic
        fun init(ctx: Context) {
            appContext = ctx
        }

        @JvmStatic
        fun setConnected(connected: Int) {
            val intent = Intent(appContext, MellowKeepAliveService::class.java)
            if (connected != 0) {
                androidx.core.content.ContextCompat.startForegroundService(appContext, intent)
                maybeAskIgnoreBatteryOptimizations()
            } else {
                appContext.stopService(intent)
            }
        }

        @JvmStatic
        fun setBadge(count: String) {
            if (::appContext.isInitialized) {
                MellowKeepAliveService.updateBadge(appContext, count)
            }
        }

        /** One-tap exemption prompt — kept because we're sideloaded, not Play-published. */
        private fun maybeAskIgnoreBatteryOptimizations() {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M || batteryPromptShown) return
            if (!::appContext.isInitialized) return
            val pm = appContext.getSystemService(Context.POWER_SERVICE) as PowerManager
            if (pm.isIgnoringBatteryOptimizations(appContext.packageName)) return
            batteryPromptShown = true
            try {
                val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                    data = Uri.parse("package:" + appContext.packageName)
                    flags = Intent.FLAG_ACTIVITY_NEW_TASK
                }
                appContext.startActivity(intent)
            } catch (_: Exception) {
                // OEM ROMs are free to hide this screen; the FGS still works.
            }
        }
    }
}
