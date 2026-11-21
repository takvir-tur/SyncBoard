package com.clipsync.android

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.widget.Toast

class ShareReceiverActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        if (intent?.action == Intent.ACTION_SEND) {
            handleSend(intent)
        }
        finish()
        overridePendingTransition(0, 0)
    }

    private fun handleSend(intent: Intent) {
        val app = ClipSyncApp.instance
        val type = intent.type ?: return

        if (type.startsWith("text/")) {
            val text = intent.getStringExtra(Intent.EXTRA_TEXT) ?: return
            val rc = app.pushLocal(ClipSyncNative.KIND_TEXT, text.toByteArray(Charsets.UTF_8))
            if (rc >= 0) {
                Toast.makeText(this, "Sent text to ClipSync", Toast.LENGTH_SHORT).show()
            }
        } else if (type.startsWith("image/")) {
            val uri: Uri? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableExtra(Intent.EXTRA_STREAM)
            }
            if (uri != null) {
                val bytes = app.readUriBytes(this, uri) ?: return
                val rc = app.pushLocal(ClipSyncNative.KIND_IMAGE, bytes)
                if (rc >= 0) {
                    Toast.makeText(this, "Sent image to ClipSync", Toast.LENGTH_SHORT).show()
                }
            }
        }
    }
}
