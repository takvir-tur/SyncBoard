package com.clipsync.android

import java.nio.ByteBuffer
import java.nio.ByteOrder

object ClipSyncNative {
    const val KIND_TEXT = 1
    const val KIND_IMAGE = 2

    init {
        System.loadLibrary("clipsync_core")
    }

    data class RemoteClip(
        val kind: Int,
        val timestampMs: Long,
        val mime: String,
        val data: ByteArray,
    )

    data class PinItem(
        val id: ByteArray,
        val kind: Int,
        val createdAtMs: Long,
        val updatedAtMs: Long,
        val mime: String,
        val data: ByteArray,
    )

    external fun nativeInit(dataDir: String, relayUrl: String?): Long
    external fun nativePushLocal(handle: Long, kind: Int, data: ByteArray): Int
    external fun nativePopRemote(handle: Long): ByteArray?
    external fun nativeWaitEvent(handle: Long, timeoutMs: Long): Boolean
    external fun nativePinItem(handle: Long, kind: Int, data: ByteArray): ByteArray?
    external fun nativeUnpinItem(handle: Long, idBytes: ByteArray): Boolean
    external fun nativeGetPins(handle: Long): ByteArray?
    external fun nativeGetPairingUri(handle: Long): String?
    external fun nativeJoinPairingUri(handle: Long, uri: String): Boolean
    external fun nativeRecordEcho(handle: Long, kind: Int, data: ByteArray)
    external fun nativeShutdown(handle: Long)

    fun decodeRemoteClip(raw: ByteArray?): RemoteClip? {
        if (raw == null || raw.size < 17) return null
        val buf = ByteBuffer.wrap(raw).order(ByteOrder.BIG_ENDIAN)
        val kind = buf.get().toInt() and 0xFF
        val ts = buf.getLong()
        val mimeLen = buf.getInt()
        if (mimeLen < 0 || mimeLen > buf.remaining()) return null
        val mimeBytes = ByteArray(mimeLen)
        buf.get(mimeBytes)
        if (buf.remaining() < 4) return null
        val dataLen = buf.getInt()
        if (dataLen < 0 || dataLen > buf.remaining()) return null
        val data = ByteArray(dataLen)
        buf.get(data)
        return RemoteClip(
            kind = kind,
            timestampMs = ts,
            mime = String(mimeBytes, Charsets.UTF_8),
            data = data,
        )
    }

    fun decodePins(raw: ByteArray?): List<PinItem> {
        if (raw == null || raw.size < 4) return emptyList()
        val buf = ByteBuffer.wrap(raw).order(ByteOrder.BIG_ENDIAN)
        val count = buf.getInt()
        if (count <= 0) return emptyList()
        val out = ArrayList<PinItem>(count.coerceAtMost(25))
        for (i in 0 until count) {
            if (buf.remaining() < 16 + 1 + 8 + 8 + 4) break
            val id = ByteArray(16)
            buf.get(id)
            val kind = buf.get().toInt() and 0xFF
            val createdAt = buf.getLong()
            val updatedAt = buf.getLong()
            val mimeLen = buf.getInt()
            if (mimeLen < 0 || mimeLen > buf.remaining()) break
            val mimeBytes = ByteArray(mimeLen)
            buf.get(mimeBytes)
            if (buf.remaining() < 4) break
            val dataLen = buf.getInt()
            if (dataLen < 0 || dataLen > buf.remaining()) break
            val data = ByteArray(dataLen)
            buf.get(data)
            out.add(
                PinItem(
                    id = id,
                    kind = kind,
                    createdAtMs = createdAt,
                    updatedAtMs = updatedAt,
                    mime = String(mimeBytes, Charsets.UTF_8),
                    data = data,
                )
            )
        }
        return out
    }
}
