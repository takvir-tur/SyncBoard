package com.clipsync.android

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.LayoutInflater
import android.view.View
import android.view.ViewGroup
import android.widget.EditText
import android.widget.Toast
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.ContextCompat
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.clipsync.android.databinding.ActivityMainBinding
import com.clipsync.android.databinding.ItemPinBinding
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.zxing.BarcodeFormat
import com.google.zxing.qrcode.QRCodeWriter

class MainActivity : AppCompatActivity() {
    private lateinit var binding: ActivityMainBinding
    private lateinit var pinAdapter: PinAdapter

    private val app: ClipSyncApp
        get() = ClipSyncApp.instance

    private val stateListener: () -> Unit = {
        runOnUiThread { refreshUi() }
    }

    private val notificationPermLauncher = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { _ ->
        SyncForegroundService.start(this)
        refreshUi()
    }

    private val qrScanLauncher = registerForActivityResult(
        ActivityResultContracts.StartActivityForResult()
    ) { result ->
        val uri = result.data?.getStringExtra(QrScannerActivity.EXTRA_QR_URI) ?: return@registerForActivityResult
        joinClusterUri(uri)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        binding = ActivityMainBinding.inflate(layoutInflater)
        setContentView(binding.root)

        pinAdapter = PinAdapter(
            onCopy = { pin ->
                if (app.writeToClipboard(this, pin.kind, pin.mime, pin.data)) {
                    Toast.makeText(this, "Pin copied to clipboard", Toast.LENGTH_SHORT).show()
                }
            },
            onDelete = { pin ->
                app.unpinItem(pin.id)
            }
        )

        binding.rvPins.layoutManager = LinearLayoutManager(this)
        binding.rvPins.adapter = pinAdapter

        binding.btnToggleSync.setOnClickListener {
            if (SyncForegroundService.isRunning) {
                SyncForegroundService.stop(this)
            } else {
                ensureNotificationPermissionAndStart()
            }
            refreshUi()
        }

        binding.btnSyncNow.setOnClickListener {
            val clip = app.readCurrentClipboard(this)
            if (clip == null) {
                Toast.makeText(this, "Clipboard empty or sensitive", Toast.LENGTH_SHORT).show()
            } else {
                val rc = app.pushLocal(clip.first, clip.second)
                val msg = if (rc == 1) "Synced clipboard" else "Already synced"
                Toast.makeText(this, msg, Toast.LENGTH_SHORT).show()
            }
        }

        binding.btnPinNow.setOnClickListener {
            val clip = app.readCurrentClipboard(this)
            if (clip == null) {
                Toast.makeText(this, "Clipboard empty or sensitive", Toast.LENGTH_SHORT).show()
            } else {
                val id = app.pinItem(clip.first, clip.second)
                val msg = if (id != null) "Pinned clipboard" else "Failed to pin"
                Toast.makeText(this, msg, Toast.LENGTH_SHORT).show()
            }
        }

        binding.btnOverlayPermission.setOnClickListener {
            val intent = Intent(
                Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                Uri.parse("package:$packageName")
            )
            startActivity(intent)
        }

        binding.btnAccessibilityPermission.setOnClickListener {
            startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
        }

        binding.btnOpenAppInfo.setOnClickListener {
            val intent = Intent(
                Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                Uri.parse("package:$packageName")
            )
            startActivity(intent)
        }

        binding.btnCopyUri.setOnClickListener {
            val uri = app.getPairingUri()
            app.writeToClipboard(this, ClipSyncNative.KIND_TEXT, "text/plain", uri.toByteArray(Charsets.UTF_8))
            Toast.makeText(this, "Pairing URI copied", Toast.LENGTH_SHORT).show()
        }

        binding.btnScanQr.setOnClickListener {
            qrScanLauncher.launch(Intent(this, QrScannerActivity::class.java))
        }

        binding.btnJoinUri.setOnClickListener {
            showJoinDialog()
        }

        ensureNotificationPermissionAndStart()
        handleIncomingDeepLink(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleIncomingDeepLink(intent)
    }

    override fun onStart() {
        super.onStart()
        app.addStateListener(stateListener)
        refreshUi()
    }

    override fun onResume() {
        super.onResume()
        SyncForegroundService.refreshStatus(this)
        refreshUi()
    }

    override fun onStop() {
        app.removeStateListener(stateListener)
        super.onStop()
    }

    private fun ensureNotificationPermissionAndStart() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val granted = ContextCompat.checkSelfPermission(
                this,
                Manifest.permission.POST_NOTIFICATIONS
            ) == PackageManager.PERMISSION_GRANTED
            if (!granted) {
                notificationPermLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
                return
            }
        }
        SyncForegroundService.start(this)
    }

    private fun handleIncomingDeepLink(intent: Intent?) {
        val data = intent?.data ?: return
        if (data.scheme == "clipsync" && data.host == "pair") {
            joinClusterUri(data.toString())
        }
    }

    private fun joinClusterUri(uri: String) {
        if (app.joinPairingUri(uri)) {
            Toast.makeText(this, "Joined ClipSync cluster", Toast.LENGTH_SHORT).show()
            refreshUi()
        } else {
            Toast.makeText(this, "Invalid clipsync://pair URI", Toast.LENGTH_SHORT).show()
        }
    }

    private fun showJoinDialog() {
        val input = EditText(this).apply {
            hint = "clipsync://pair?k=...&r=..."
        }
        MaterialAlertDialogBuilder(this)
            .setTitle("Join Cluster")
            .setView(input)
            .setPositiveButton("Join") { _, _ ->
                joinClusterUri(input.text.toString().trim())
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun refreshUi() {
        binding.btnToggleSync.text = if (SyncForegroundService.isRunning) "Stop Sync" else "Start Sync"

        val overlayGranted = Settings.canDrawOverlays(this)
        val accessibilityEnabled = ClipSyncAccessibilityService.isEnabled(this)

        binding.btnOverlayPermission.text = if (overlayGranted) {
            "✓ 1. Display Over Other Apps"
        } else {
            "1. Allow Display Over Other Apps"
        }
        binding.btnAccessibilityPermission.text = if (accessibilityEnabled) {
            "✓ 2. Accessibility Auto-Sync Enabled"
        } else {
            "2. Enable ClipSync Accessibility"
        }

        binding.tvAutoSyncStatus.text = when {
            overlayGranted && accessibilityEnabled ->
                "Active: copying text or screenshots syncs automatically"
            overlayGranted ->
                "Step 1 complete — enable ClipSync in Accessibility Settings"
            accessibilityEnabled ->
                "Step 2 complete — allow Display Over Other Apps"
            else ->
                "Optional: complete both steps for zero-tap background sync (Quick Settings tile & Share Sheet work anytime)"
        }

        val uri = app.getPairingUri()
        binding.tvPairingUri.text = uri
        binding.ivQrCode.setImageBitmap(renderQrBitmap(uri, 512))

        val pins = app.getPins()
        binding.tvEmptyPins.visibility = if (pins.isEmpty()) View.VISIBLE else View.GONE
        binding.tvPinsHeader.text = "Pinned Items (${pins.size})"
        pinAdapter.submitList(pins)
    }

    private fun renderQrBitmap(content: String, sizePx: Int): Bitmap? {
        if (content.isEmpty()) return null
        return try {
            val matrix = QRCodeWriter().encode(content, BarcodeFormat.QR_CODE, sizePx, sizePx)
            val pixels = IntArray(sizePx * sizePx)
            for (y in 0 until sizePx) {
                val offset = y * sizePx
                for (x in 0 until sizePx) {
                    pixels[offset + x] = if (matrix[x, y]) Color.BLACK else Color.WHITE
                }
            }
            Bitmap.createBitmap(pixels, sizePx, sizePx, Bitmap.Config.ARGB_8888)
        } catch (_: Exception) {
            null
        }
    }

    private class PinAdapter(
        private val onCopy: (ClipSyncNative.PinItem) -> Unit,
        private val onDelete: (ClipSyncNative.PinItem) -> Unit,
    ) : RecyclerView.Adapter<PinAdapter.VH>() {

        private var items: List<ClipSyncNative.PinItem> = emptyList()

        fun submitList(newItems: List<ClipSyncNative.PinItem>) {
            items = newItems
            notifyDataSetChanged()
        }

        class VH(val binding: ItemPinBinding) : RecyclerView.ViewHolder(binding.root)

        override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): VH {
            val binding = ItemPinBinding.inflate(LayoutInflater.from(parent.context), parent, false)
            return VH(binding)
        }

        override fun getItemCount(): Int = items.size

        override fun onBindViewHolder(holder: VH, position: Int) {
            val item = items[position]
            val b = holder.binding

            if (item.kind == ClipSyncNative.KIND_IMAGE) {
                val bmp = BitmapFactory.decodeByteArray(item.data, 0, item.data.size)
                b.ivThumb.visibility = View.VISIBLE
                b.ivThumb.setImageBitmap(bmp)
                b.tvContent.text = "Image (${(item.data.size + 1023) / 1024} KB)"
                b.tvMeta.text = item.mime
            } else {
                b.ivThumb.visibility = View.GONE
                b.ivThumb.setImageDrawable(null)
                b.tvContent.text = String(item.data, Charsets.UTF_8)
                b.tvMeta.text = "Text • ${item.data.size} B"
            }

            b.root.setOnClickListener { onCopy(item) }
            b.btnDelete.setOnClickListener { onDelete(item) }
        }
    }
}
