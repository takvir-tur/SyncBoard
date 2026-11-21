package com.clipsync.android

import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import android.os.Build
import androidx.core.content.FileProvider
import java.io.File
import java.util.concurrent.CopyOnWriteArraySet

class ClipSyncApp : Application() {
    companion object {
        const val DEFAULT_RELAY_URL = "wss://clipsync-relay.takvir-tur.workers.dev"
        const val CHANNEL_ID = "clipsync_sync_channel"
        private const val EXTRA_IS_SENSITIVE = "android.content.extra.IS_SENSITIVE"
        private const val MAX_READ_BYTES = 16 * 1024 * 1024
        private const val PREFS_NAME = "clipsync_prefs"
        private const val KEY_AUTOSTART = "autostart_enabled"

        @Volatile
        lateinit var instance: ClipSyncApp
            private set

        fun isAutoStartEnabled(context: Context): Boolean {
            return context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .getBoolean(KEY_AUTOSTART, true)
        }

        fun setAutoStartEnabled(context: Context, enabled: Boolean) {
            context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
                .edit()
                .putBoolean(KEY_AUTOSTART, enabled)
                .apply()
        }
    }

    @Volatile
    var engineHandle: Long = 0L
        private set

    private val listeners = CopyOnWriteArraySet<() -> Unit>()

    override fun onCreate() {
        super.onCreate()
        instance = this
        createNotificationChannel()

        val dataDir = File(filesDir, "clipsync")
        dataDir.mkdirs()
        engineHandle = ClipSyncNative.nativeInit(dataDir.absolutePath, DEFAULT_RELAY_URL)
    }

    override fun onTerminate() {
        val h = engineHandle
        engineHandle = 0L
        if (h != 0L) {
            ClipSyncNative.nativeShutdown(h)
        }
        super.onTerminate()
    }

    fun addStateListener(listener: () -> Unit) {
        listeners.add(listener)
    }

    fun removeStateListener(listener: () -> Unit) {
        listeners.remove(listener)
    }

    fun notifyStateChanged() {
        for (listener in listeners) {
            listener()
        }
    }

    fun pushLocal(kind: Int, data: ByteArray): Int {
        val h = engineHandle
        if (h == 0L || data.isEmpty()) return -1
        return ClipSyncNative.nativePushLocal(h, kind, data)
    }

    fun popRemote(): ClipSyncNative.RemoteClip? {
        val h = engineHandle
        if (h == 0L) return null
        return ClipSyncNative.decodeRemoteClip(ClipSyncNative.nativePopRemote(h))
    }

    fun pinItem(kind: Int, data: ByteArray): ByteArray? {
        val h = engineHandle
        if (h == 0L || data.isEmpty()) return null
        val id = ClipSyncNative.nativePinItem(h, kind, data)
        notifyStateChanged()
        return id
    }

    fun unpinItem(id: ByteArray): Boolean {
        val h = engineHandle
        if (h == 0L) return false
        val ok = ClipSyncNative.nativeUnpinItem(h, id)
        if (ok) notifyStateChanged()
        return ok
    }

    fun getPins(): List<ClipSyncNative.PinItem> {
        val h = engineHandle
        if (h == 0L) return emptyList()
        return ClipSyncNative.decodePins(ClipSyncNative.nativeGetPins(h))
    }

    fun getPairingUri(): String {
        val h = engineHandle
        if (h == 0L) return ""
        return ClipSyncNative.nativeGetPairingUri(h) ?: ""
    }

    fun joinPairingUri(uri: String): Boolean {
        val h = engineHandle
        if (h == 0L) return false
        val ok = ClipSyncNative.nativeJoinPairingUri(h, uri)
        if (ok) notifyStateChanged()
        return ok
    }

    fun recordEcho(kind: Int, data: ByteArray) {
        val h = engineHandle
        if (h != 0L && data.isNotEmpty()) {
            ClipSyncNative.nativeRecordEcho(h, kind, data)
        }
    }

    fun readCurrentClipboard(context: Context): Pair<Int, ByteArray>? {
        val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return null
        return extractClipFromClipData(context, cm.primaryClip)
    }

    fun extractClipFromClipData(context: Context, clip: ClipData?): Pair<Int, ByteArray>? {
        if (clip == null || clip.itemCount == 0) return null

        val desc = clip.description
        if (desc?.extras?.getBoolean(EXTRA_IS_SENSITIVE, false) == true) {
            return null
        }

        val item = clip.getItemAt(0)
        val uri = item.uri
        if (uri != null) {
            val mime = context.contentResolver.getType(uri)
            if (mime?.startsWith("image/") == true) {
                val bytes = readUriBytes(context, uri)
                if (bytes != null && bytes.isNotEmpty()) {
                    return ClipSyncNative.KIND_IMAGE to bytes
                }
            }
        }

        val text = item.coerceToText(context)?.toString()
        if (!text.isNullOrEmpty()) {
            return ClipSyncNative.KIND_TEXT to text.toByteArray(Charsets.UTF_8)
        }
        return null
    }

    fun writeToClipboard(context: Context, kind: Int, mime: String, data: ByteArray): Boolean {
        val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return false
        recordEcho(kind, data)

        return when (kind) {
            ClipSyncNative.KIND_TEXT -> {
                val text = String(data, Charsets.UTF_8)
                cm.setPrimaryClip(ClipData.newPlainText("ClipSync", text))
                true
            }
            ClipSyncNative.KIND_IMAGE -> {
                val dir = File(context.cacheDir, "clipboard").apply { mkdirs() }
                pruneOldCacheFiles(dir)
                val ext = when {
                    mime.contains("png") -> "png"
                    mime.contains("webp") -> "webp"
                    else -> "jpg"
                }
                val file = File(dir, "clip_${System.currentTimeMillis()}.$ext")
                file.writeBytes(data)
                val contentUri: Uri = FileProvider.getUriForFile(
                    context,
                    "${context.packageName}.fileprovider",
                    file
                )
                cm.setPrimaryClip(ClipData.newUri(context.contentResolver, "ClipSync Image", contentUri))
                true
            }
            else -> false
        }
    }

    fun readUriBytes(context: Context, uri: Uri): ByteArray? {
        return try {
            context.contentResolver.openInputStream(uri)?.use { stream ->
                val raw = stream.readBytes()
                if (raw.size <= MAX_READ_BYTES) raw else null
            }
        } catch (_: Exception) {
            null
        }
    }

    private fun pruneOldCacheFiles(dir: File) {
        val files = dir.listFiles()?.sortedByDescending { it.lastModified() } ?: return
        if (files.size > 10) {
            files.drop(10).forEach { it.delete() }
        }
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "ClipSync Background Sync",
                NotificationManager.IMPORTANCE_LOW
            ).apply {
                description = "Keeps encrypted clipboard sync active"
                setShowBadge(false)
            }
            val nm = getSystemService(NotificationManager::class.java)
            nm?.createNotificationChannel(channel)
        }
    }
}
