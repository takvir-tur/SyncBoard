package com.clipsync.android

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.Toast

class FocusCaptureActivity : Activity() {
    companion object {
        const val EXTRA_MODE = "mode"
        const val MODE_SYNC = "sync"
        const val MODE_PIN = "pin"

        fun createIntent(context: Context, mode: String): Intent {
            return Intent(context, FocusCaptureActivity::class.java).apply {
                putExtra(EXTRA_MODE, mode)
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION)
            }
        }
    }

    private val handler = Handler(Looper.getMainLooper())
    private var captured = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Safety timeout in case window focus callback is skipped.
        handler.postDelayed({ finishSafely() }, 400L)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus && !captured) {
            captured = true
            // Brief 30ms window after gaining focus so Android allows ClipboardManager read.
            handler.postDelayed({
                performCapture()
                finishSafely()
            }, 30L)
        }
    }

    private fun performCapture() {
        val app = ClipSyncApp.instance
        val clip = app.readCurrentClipboard(this)
        if (clip == null) {
            Toast.makeText(this, "Clipboard empty or sensitive", Toast.LENGTH_SHORT).show()
            return
        }

        val mode = intent?.getStringExtra(EXTRA_MODE) ?: MODE_SYNC
        if (mode == MODE_PIN) {
            val id = app.pinItem(clip.first, clip.second)
            val msg = if (id != null) "Pinned to ClipSync" else "Failed to pin item"
            Toast.makeText(this, msg, Toast.LENGTH_SHORT).show()
        } else {
            val rc = app.pushLocal(clip.first, clip.second)
            val msg = when (rc) {
                1 -> "Synced clipboard to cluster"
                0 -> "Already up to date"
                else -> "Sync failed"
            }
            Toast.makeText(this, msg, Toast.LENGTH_SHORT).show()
        }
    }

    private fun finishSafely() {
        if (!isFinishing) {
            finish()
            overridePendingTransition(0, 0)
        }
    }

    override fun onDestroy() {
        handler.removeCallbacksAndMessages(null)
        super.onDestroy()
    }
}
