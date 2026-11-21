package com.clipsync.android

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

class BootCompletedReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent?) {
        val action = intent?.action ?: return
        if (action != Intent.ACTION_BOOT_COMPLETED &&
            action != Intent.ACTION_MY_PACKAGE_REPLACED
        ) {
            return
        }
        if (!ClipSyncApp.isAutoStartEnabled(context)) {
            return
        }
        try {
            SyncForegroundService.start(context)
        } catch (_: Throwable) {
            // Android 15+ may restrict starting dataSync FGS directly from BOOT_COMPLETED;
            // the app remains ready via Quick Settings tile, Share Sheet, or app launch.
        }
    }
}
