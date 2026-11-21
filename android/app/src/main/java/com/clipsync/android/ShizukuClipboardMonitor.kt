package com.clipsync.android

import android.content.ClipData
import android.content.Context
import android.content.pm.PackageManager
import android.os.Binder
import android.os.IBinder
import android.os.Parcel
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import rikka.shizuku.Shizuku
import rikka.shizuku.ShizukuBinderWrapper
import rikka.shizuku.SystemServiceHelper
import java.lang.reflect.Proxy

class ShizukuClipboardMonitor(
    private val context: Context,
    private val scope: CoroutineScope,
    private val onStatusChanged: () -> Unit,
) {
    companion object {
        private const val LISTENER_DESCRIPTOR = "android.content.IOnPrimaryClipChangedListener"

        fun isShizukuAvailable(): Boolean {
            return try {
                Shizuku.pingBinder()
            } catch (_: Throwable) {
                false
            }
        }

        fun hasPermission(): Boolean {
            return try {
                isShizukuAvailable() && Shizuku.checkSelfPermission() == PackageManager.PERMISSION_GRANTED
            } catch (_: Throwable) {
                false
            }
        }
    }

    @Volatile
    var isAttached: Boolean = false
        private set

    private var clipboardService: Any? = null
    private var listenerProxy: Any? = null
    private var started = false

    private val binderReceivedListener = Shizuku.OnBinderReceivedListener {
        if (ClipSyncApp.isShizukuEnabled(context)) {
            attach()
        }
        onStatusChanged()
    }

    private val binderDeadListener = Shizuku.OnBinderDeadListener {
        isAttached = false
        clipboardService = null
        listenerProxy = null
        onStatusChanged()
    }

    fun start() {
        if (started) {
            if (!isAttached && ClipSyncApp.isShizukuEnabled(context)) {
                attach()
            }
            return
        }
        started = true
        try {
            Shizuku.addBinderReceivedListenerSticky(binderReceivedListener)
            Shizuku.addBinderDeadListener(binderDeadListener)
        } catch (_: Throwable) {
            started = false
            return
        }
        if (ClipSyncApp.isShizukuEnabled(context)) {
            attach()
        }
    }

    fun stop() {
        detach()
        if (!started) return
        started = false
        try {
            Shizuku.removeBinderReceivedListener(binderReceivedListener)
            Shizuku.removeBinderDeadListener(binderDeadListener)
        } catch (_: Throwable) {
            // Ignore if Shizuku provider was unavailable.
        }
    }

    @Synchronized
    fun attach(): Boolean {
        if (isAttached) return true
        if (!hasPermission()) return false

        return try {
            val rawBinder = SystemServiceHelper.getSystemService(Context.CLIPBOARD_SERVICE)
                ?: return false
            val wrappedBinder = ShizukuBinderWrapper(rawBinder)
            val stubClass = Class.forName("android.content.IClipboard\$Stub")
            val asInterface = stubClass.getMethod("asInterface", IBinder::class.java)
            val service = asInterface.invoke(null, wrappedBinder) ?: return false

            val listenerInterface = Class.forName(LISTENER_DESCRIPTOR)
            val callbackBinder = object : Binder() {
                override fun onTransact(code: Int, data: Parcel, reply: Parcel?, flags: Int): Boolean {
                    return when (code) {
                        INTERFACE_TRANSACTION -> {
                            reply?.writeString(LISTENER_DESCRIPTOR)
                            true
                        }
                        FIRST_CALL_TRANSACTION -> {
                            data.enforceInterface(LISTENER_DESCRIPTOR)
                            handleRemotePrimaryClipChanged()
                            reply?.writeNoException()
                            true
                        }
                        else -> super.onTransact(code, data, reply, flags)
                    }
                }
            }

            val proxy = Proxy.newProxyInstance(
                listenerInterface.classLoader,
                arrayOf(listenerInterface)
            ) { _, method, _ ->
                when (method.name) {
                    "asBinder" -> callbackBinder
                    "dispatchPrimaryClipChanged" -> {
                        handleRemotePrimaryClipChanged()
                        null
                    }
                    else -> null
                }
            }

            val registered = invokeClipboardMethod(
                service = service,
                methodName = "addPrimaryClipChangedListener",
                listenerInterface = listenerInterface,
                listenerObj = proxy
            )
            if (registered.first) {
                clipboardService = service
                listenerProxy = proxy
                isAttached = true
                onStatusChanged()
                true
            } else {
                false
            }
        } catch (_: Throwable) {
            isAttached = false
            false
        }
    }

    @Synchronized
    fun detach() {
        val service = clipboardService
        val proxy = listenerProxy
        clipboardService = null
        listenerProxy = null
        val wasAttached = isAttached
        isAttached = false

        if (service != null && proxy != null) {
            try {
                val listenerInterface = Class.forName(LISTENER_DESCRIPTOR)
                invokeClipboardMethod(
                    service = service,
                    methodName = "removePrimaryClipChangedListener",
                    listenerInterface = listenerInterface,
                    listenerObj = proxy
                )
            } catch (_: Throwable) {
                // Ignore if service binder already died.
            }
        }
        if (wasAttached) {
            onStatusChanged()
        }
    }

    private fun handleRemotePrimaryClipChanged() {
        val service = clipboardService ?: return
        scope.launch(Dispatchers.IO) {
            try {
                val listenerInterface = Class.forName(LISTENER_DESCRIPTOR)
                val (ok, result) = invokeClipboardMethod(
                    service = service,
                    methodName = "getPrimaryClip",
                    listenerInterface = listenerInterface,
                    listenerObj = null
                )
                if (!ok) return@launch
                val clipData = result as? ClipData ?: return@launch
                val app = ClipSyncApp.instance
                val extracted = app.extractClipFromClipData(context, clipData) ?: return@launch
                app.pushLocal(extracted.first, extracted.second)
            } catch (_: Throwable) {
                // Ignore transient IPC errors cleanly.
            }
        }
    }

    private fun invokeClipboardMethod(
        service: Any,
        methodName: String,
        listenerInterface: Class<*>,
        listenerObj: Any?,
    ): Pair<Boolean, Any?> {
        val methods = service.javaClass.methods.filter { it.name == methodName }
        val userId = currentUserId()
        val packages = try {
            if (Shizuku.getUid() == 2000) {
                listOf("com.android.shell", context.packageName)
            } else {
                listOf(context.packageName, "com.android.shell")
            }
        } catch (_: Throwable) {
            listOf("com.android.shell", context.packageName)
        }

        for (pkg in packages) {
            for (method in methods) {
                val args = buildArgs(method.parameterTypes, listenerInterface, listenerObj, pkg, userId)
                    ?: continue
                try {
                    return true to method.invoke(service, *args)
                } catch (_: Throwable) {
                    // Try next signature or calling package.
                }
            }
        }
        return false to null
    }

    private fun buildArgs(
        paramTypes: Array<Class<*>>,
        listenerInterface: Class<*>,
        listenerObj: Any?,
        pkg: String,
        userId: Int,
    ): Array<Any?>? {
        val args = arrayOfNulls<Any?>(paramTypes.size)
        var strCount = 0
        var intCount = 0
        var usedListener = false

        for (i in paramTypes.indices) {
            val type = paramTypes[i]
            when {
                listenerObj != null && type.isAssignableFrom(listenerInterface) -> {
                    args[i] = listenerObj
                    usedListener = true
                }
                type == String::class.java -> {
                    args[i] = if (strCount == 0) pkg else null
                    strCount++
                }
                type == Int::class.javaPrimitiveType || type == Integer::class.java -> {
                    args[i] = if (intCount == 0) userId else 0
                    intCount++
                }
                else -> return null
            }
        }

        if (listenerObj != null && !usedListener) {
            return null
        }
        return args
    }

    private fun currentUserId(): Int {
        return try {
            val method = android.os.UserHandle::class.java.getMethod("myUserId")
            (method.invoke(null) as? Int) ?: 0
        } catch (_: Throwable) {
            0
        }
    }
}
