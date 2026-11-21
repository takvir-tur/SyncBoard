package com.clipsync.android

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

class SyncForegroundService : Service() {
    companion object {
        private const val NOTIFICATION_ID = 1001
        private const val ACTION_REFRESH_SHIZUKU = "com.clipsync.android.REFRESH_SHIZUKU"

        @Volatile
        var isRunning: Boolean = false
            private set

        @Volatile
        var isShizukuActive: Boolean = false
            private set

        fun start(context: Context) {
            val intent = Intent(context, SyncForegroundService::class.java)
            ContextCompat.startForegroundService(context, intent)
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, SyncForegroundService::class.java))
        }

        fun refreshShizuku(context: Context) {
            if (!isRunning) return
            val intent = Intent(context, SyncForegroundService::class.java).apply {
                action = ACTION_REFRESH_SHIZUKU
            }
            ContextCompat.startForegroundService(context, intent)
        }
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var workerJob: Job? = null
    private var shizukuMonitor: ShizukuClipboardMonitor? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        isRunning = true

        if (shizukuMonitor == null) {
            shizukuMonitor = ShizukuClipboardMonitor(this, scope) {
                isShizukuActive = shizukuMonitor?.isAttached == true
                updateNotification()
                ClipSyncApp.instance.notifyStateChanged()
            }
        }

        if (ClipSyncApp.isShizukuEnabled(this)) {
            shizukuMonitor?.start()
        } else {
            shizukuMonitor?.stop()
        }
        isShizukuActive = shizukuMonitor?.isAttached == true

        val notification = buildNotification()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ServiceCompat.startForeground(
                this,
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }

        if (workerJob == null || workerJob?.isActive != true) {
            workerJob = scope.launch {
                val app = ClipSyncApp.instance
                while (isActive) {
                    val handle = app.engineHandle
                    if (handle == 0L) break

                    ClipSyncNative.nativeWaitEvent(handle, 5000L)
                    var appliedRemote = false
                    while (true) {
                        val clip = app.popRemote() ?: break
                        app.writeToClipboard(this@SyncForegroundService, clip.kind, clip.mime, clip.data)
                        appliedRemote = true
                    }
                    if (appliedRemote || isActive) {
                        app.notifyStateChanged()
                    }
                }
            }
        }
        return START_STICKY
    }

    override fun onDestroy() {
        isRunning = false
        isShizukuActive = false
        shizukuMonitor?.stop()
        shizukuMonitor = null
        workerJob?.cancel()
        scope.cancel()
        ClipSyncApp.instance.notifyStateChanged()
        super.onDestroy()
    }

    private fun updateNotification() {
        if (!isRunning) return
        val nm = getSystemService(NotificationManager::class.java)
        nm?.notify(NOTIFICATION_ID, buildNotification())
    }

    private fun buildNotification(): Notification {
        val openAppIntent = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

        val syncIntent = PendingIntent.getActivity(
            this,
            1,
            FocusCaptureActivity.createIntent(this, FocusCaptureActivity.MODE_SYNC),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

        val pinIntent = PendingIntent.getActivity(
            this,
            2,
            FocusCaptureActivity.createIntent(this, FocusCaptureActivity.MODE_PIN),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

        val statusText = if (isShizukuActive) {
            "Shizuku auto-sync active — copies push automatically"
        } else {
            "Tap Sync or Pin to capture Android clipboard"
        }

        return NotificationCompat.Builder(this, ClipSyncApp.CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_clipsync)
            .setContentTitle("ClipSync Active")
            .setContentText(statusText)
            .setContentIntent(openAppIntent)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .addAction(0, getString(R.string.action_sync), syncIntent)
            .addAction(0, getString(R.string.action_pin), pinIntent)
            .build()
    }
}
