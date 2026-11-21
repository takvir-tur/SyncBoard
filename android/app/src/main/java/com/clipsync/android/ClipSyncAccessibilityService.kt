package com.clipsync.android

import android.accessibilityservice.AccessibilityService
import android.content.ClipboardManager
import android.content.ComponentName
import android.content.Context
import android.graphics.PixelFormat
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.view.Gravity
import android.view.View
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import java.util.concurrent.Executors

class ClipSyncAccessibilityService : AccessibilityService() {
    companion object {
        private const val DEBOUNCE_MS = 350L
        private const val SELECTION_COOLDOWN_MS = 900L
        private const val FOCUS_DELAY_MS = 30L
        private const val SAFETY_TIMEOUT_MS = 250L

        @Volatile
        var isServiceRunning: Boolean = false
            private set

        fun isEnabled(context: Context): Boolean {
            if (isServiceRunning) return true
            val enabledServices = Settings.Secure.getString(
                context.contentResolver,
                Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES
            ) ?: return false
            val full = ComponentName(context, ClipSyncAccessibilityService::class.java).flattenToString()
            val short = ComponentName(context, ClipSyncAccessibilityService::class.java).flattenToShortString()
            return enabledServices.split(':').any {
                it.equals(full, ignoreCase = true) || it.equals(short, ignoreCase = true)
            }
        }

        fun isReady(context: Context): Boolean {
            return Settings.canDrawOverlays(context) && isEnabled(context)
        }
    }

    private val mainHandler = Handler(Looper.getMainLooper())
    private val ioExecutor = Executors.newSingleThreadExecutor()
    private var activeOverlay: View? = null
    private var isCapturing = false
    private var lastCaptureElapsedMs = 0L

    private val captureRunnable = Runnable {
        performOverlayCapture()
    }

    private val safetyTimeoutRunnable = Runnable {
        removeOverlay()
        isCapturing = false
    }

    override fun onServiceConnected() {
        super.onServiceConnected()
        isServiceRunning = true
        SyncForegroundService.refreshStatus(this)
        ClipSyncApp.instance.notifyStateChanged()
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event == null) return
        val pkg = event.packageName?.toString()
        if (pkg == packageName) return
        if (!Settings.canDrawOverlays(this)) return

        val shouldSchedule = when (event.eventType) {
            AccessibilityEvent.TYPE_ANNOUNCEMENT,
            AccessibilityEvent.TYPE_NOTIFICATION_STATE_CHANGED -> {
                pkg == "com.android.systemui" || containsCopyCue(event)
            }
            AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED -> {
                pkg == "com.android.systemui" || containsCopyCue(event)
            }
            AccessibilityEvent.TYPE_VIEW_TEXT_SELECTION_CHANGED -> {
                val elapsed = SystemClock.elapsedRealtime() - lastCaptureElapsedMs
                elapsed >= SELECTION_COOLDOWN_MS
            }
            else -> false
        }

        if (shouldSchedule) {
            mainHandler.removeCallbacks(captureRunnable)
            mainHandler.postDelayed(captureRunnable, DEBOUNCE_MS)
        }
    }

    override fun onInterrupt() {
        mainHandler.removeCallbacks(captureRunnable)
        removeOverlay()
        isCapturing = false
    }

    override fun onDestroy() {
        isServiceRunning = false
        mainHandler.removeCallbacksAndMessages(null)
        removeOverlay()
        isCapturing = false
        ioExecutor.shutdownNow()
        SyncForegroundService.refreshStatus(this)
        ClipSyncApp.instance.notifyStateChanged()
        super.onDestroy()
    }

    private fun containsCopyCue(event: AccessibilityEvent): Boolean {
        val desc = event.contentDescription?.toString()
        if (desc != null && isCopyCueText(desc)) {
            return true
        }
        for (item in event.text) {
            val s = item?.toString() ?: continue
            if (isCopyCueText(s)) {
                return true
            }
        }
        return false
    }

    private fun isCopyCueText(text: String): Boolean {
        val lower = text.lowercase()
        return lower.contains("copied") ||
            lower.contains("clipboard") ||
            lower.contains("screenshot") ||
            lower.contains("copy")
    }

    private fun performOverlayCapture() {
        if (isCapturing || !Settings.canDrawOverlays(this)) return
        val wm = getSystemService(Context.WINDOW_SERVICE) as? WindowManager ?: return

        isCapturing = true
        lastCaptureElapsedMs = SystemClock.elapsedRealtime()

        val focusView = object : View(this) {
            private var captured = false

            override fun onWindowFocusChanged(hasWindowFocus: Boolean) {
                super.onWindowFocusChanged(hasWindowFocus)
                if (hasWindowFocus && !captured) {
                    captured = true
                    mainHandler.postDelayed({
                        try {
                            captureAndPushClipboard()
                        } finally {
                            mainHandler.removeCallbacks(safetyTimeoutRunnable)
                            removeOverlay()
                            isCapturing = false
                        }
                    }, FOCUS_DELAY_MS)
                }
            }
        }

        val params = WindowManager.LayoutParams(
            1,
            1,
            WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
            PixelFormat.TRANSLUCENT
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            x = 0
            y = 0
        }

        try {
            wm.addView(focusView, params)
            activeOverlay = focusView
            mainHandler.postDelayed(safetyTimeoutRunnable, SAFETY_TIMEOUT_MS)
        } catch (_: Throwable) {
            activeOverlay = null
            isCapturing = false
        }
    }

    private fun captureAndPushClipboard() {
        val cm = getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return
        val app = ClipSyncApp.instance
        val extracted = app.extractClipFromClipData(applicationContext, cm.primaryClip) ?: return
        ioExecutor.execute {
            app.pushLocal(extracted.first, extracted.second)
        }
    }

    private fun removeOverlay() {
        val view = activeOverlay ?: return
        activeOverlay = null
        val wm = getSystemService(Context.WINDOW_SERVICE) as? WindowManager ?: return
        try {
            wm.removeView(view)
        } catch (_: Throwable) {
            // View was already detached.
        }
    }
}
