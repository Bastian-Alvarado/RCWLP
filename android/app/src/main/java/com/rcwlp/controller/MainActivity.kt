package com.rcwlp.controller

import android.Manifest
import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.hardware.input.InputManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.View
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.SeekBar
import android.widget.TextView
import android.widget.Toast
import rikka.shizuku.Shizuku
import kotlin.concurrent.thread

/** Lists PCs on the network, pairs with new ones, and opens the controller screen. */
class MainActivity : Activity(), InputManager.InputDeviceListener {
    private lateinit var store: HostStore
    private lateinit var controllerName: TextView
    private lateinit var controllerHint: TextView
    private lateinit var scanStatus: TextView
    private lateinit var hostList: LinearLayout
    private lateinit var backgroundStatus: TextView
    private lateinit var shizukuButton: Button
    private var found: List<FoundHost> = emptyList()
    private var scanning = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)
        store = HostStore(this)
        controllerName = findViewById(R.id.controller_name)
        controllerHint = findViewById(R.id.controller_hint)
        scanStatus = findViewById(R.id.scan_status)
        hostList = findViewById(R.id.host_list)
        findViewById<TextView>(R.id.version).text = "RCWLP ${BuildConfig.VERSION_NAME} · protocol v${Proto.VERSION}"
        findViewById<TextView>(R.id.scan_button).setOnClickListener { scan() }
        findViewById<TextView>(R.id.add_button).setOnClickListener { askForAddress() }
        findViewById<CheckBox>(R.id.ps4_mode).apply {
            isChecked = store.ps4Mode
            setOnCheckedChangeListener { _, checked ->
                store.ps4Mode = checked
                if (checked && !MotionSource(context) {}.hasGyro) toast(getString(R.string.ps4_mode_no_gyro))
            }
        }
        backgroundStatus = findViewById(R.id.background_status)
        shizukuButton = findViewById(R.id.shizuku_button)
        findViewById<CheckBox>(R.id.background_mode).apply {
            isChecked = store.backgroundMode
            setOnCheckedChangeListener { _, checked ->
                store.backgroundMode = checked
                if (checked && Build.VERSION.SDK_INT >= 33) {
                    // For the "connected" notification that keeps background mode running.
                    requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 0)
                }
                // A background connection left running would keep the controller grabbed and
                // fight the on-screen one for the PC.
                if (!checked) ControllerService.stop(this@MainActivity)
                updateBackgroundStatus()
            }
        }
        deadzoneSlider(R.id.stick_deadzone, R.id.stick_deadzone_value, store.stickDeadzone) { store.stickDeadzone = it }
        deadzoneSlider(R.id.trigger_deadzone, R.id.trigger_deadzone_value, store.triggerDeadzone) { store.triggerDeadzone = it }
        Shizuku.addBinderReceivedListenerSticky(shizukuChanged)
        Shizuku.addBinderDeadListener(shizukuChanged)
        Shizuku.addRequestPermissionResultListener(shizukuPermissionResult)
    }

    /** A 0–30% slider that saves as it moves; applies from the next connection. */
    private fun deadzoneSlider(sliderId: Int, valueId: Int, value: Int, save: (Int) -> Unit) {
        val label = findViewById<TextView>(valueId)
        fun show(v: Int) {
            label.text = if (v == 0) getString(R.string.deadzone_off) else "$v%"
        }
        show(value)
        findViewById<SeekBar>(sliderId).apply {
            max = Tuning.MAX_DEADZONE_PERCENT
            progress = value
            setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
                override fun onProgressChanged(bar: SeekBar, progress: Int, fromUser: Boolean) {
                    show(progress)
                    if (fromUser) save(progress)
                }
                override fun onStartTrackingTouch(bar: SeekBar) {}
                override fun onStopTrackingTouch(bar: SeekBar) {}
            })
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        Shizuku.removeBinderReceivedListener(shizukuChanged)
        Shizuku.removeBinderDeadListener(shizukuChanged)
        Shizuku.removeRequestPermissionResultListener(shizukuPermissionResult)
    }

    override fun onResume() {
        super.onResume()
        getSystemService(InputManager::class.java).registerInputDeviceListener(this, null)
        updateControllerLabel()
        updateBackgroundStatus()
        render()
        scan()
    }

    override fun onPause() {
        super.onPause()
        getSystemService(InputManager::class.java).unregisterInputDeviceListener(this)
    }

    private val shizukuChanged = object : Shizuku.OnBinderReceivedListener, Shizuku.OnBinderDeadListener {
        override fun onBinderReceived() = runOnUiThread { updateBackgroundStatus() }
        override fun onBinderDead() = runOnUiThread { updateBackgroundStatus() }
    }

    private val shizukuPermissionResult = Shizuku.OnRequestPermissionResultListener { _, _ ->
        runOnUiThread { updateBackgroundStatus() }
    }

    /** Explains what background mode still needs, with a button for the next step. */
    private fun updateBackgroundStatus() {
        if (!store.backgroundMode) {
            backgroundStatus.setText(R.string.background_off)
            shizukuButton.visibility = View.GONE
            return
        }
        val state = ShizukuState.current(this)
        backgroundStatus.setText(
            when (state) {
                ShizukuState.NOT_INSTALLED -> R.string.background_not_installed
                ShizukuState.NOT_RUNNING -> R.string.background_not_running
                ShizukuState.NO_PERMISSION -> R.string.background_no_permission
                ShizukuState.READY -> R.string.background_ready
            },
        )
        val (label, action) = when (state) {
            ShizukuState.NOT_INSTALLED -> R.string.get_shizuku to {
                startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("https://shizuku.rikka.app/download/")))
            }
            ShizukuState.NOT_RUNNING -> R.string.open_shizuku to {
                packageManager.getLaunchIntentForPackage(ShizukuState.PACKAGE)?.let(::startActivity)
                Unit
            }
            ShizukuState.NO_PERMISSION -> R.string.allow_shizuku to { Shizuku.requestPermission(0) }
            ShizukuState.READY -> null to {}
        }
        shizukuButton.visibility = if (label == null) View.GONE else View.VISIBLE
        if (label != null) shizukuButton.setText(label)
        shizukuButton.setOnClickListener { action() }
    }

    override fun onInputDeviceAdded(deviceId: Int) = updateControllerLabel()
    override fun onInputDeviceRemoved(deviceId: Int) = updateControllerLabel()
    override fun onInputDeviceChanged(deviceId: Int) = updateControllerLabel()

    private fun updateControllerLabel() {
        val names = GamepadMapper.connected().map { it.name }
        controllerName.text = names.joinToString("\n").ifEmpty { getString(R.string.no_controller) }
        controllerHint.visibility = if (names.isEmpty()) View.VISIBLE else View.GONE
    }

    private fun scan() {
        if (scanning) return
        scanning = true
        showScanStatus("Searching for PCs…")
        thread {
            val result = runCatching { Discovery.scan(store.hosts()) }.getOrDefault(emptyList())
            runOnUiThread {
                scanning = false
                if (isDestroyed) return@runOnUiThread
                found = result
                showScanStatus(if (result.isEmpty()) noPcsMessage(store.hosts()) else "")
                render()
            }
        }
    }

    /** The line under the PCs heading; it takes no room when there's nothing to say. */
    private fun showScanStatus(text: String) {
        scanStatus.text = text
        scanStatus.visibility = if (text.isEmpty()) View.GONE else View.VISIBLE
    }

    /** Says what to check, naming the address we tried when there's a paired PC. */
    private fun noPcsMessage(paired: List<PairedHost>): String = when (paired.size) {
        0 -> "No PCs found. Start rcwlp-host on the PC and make sure this phone is on the same Wi-Fi, or add the PC by IP."
        1 -> "No answer from ${paired[0].name} at ${paired[0].address}. Is rcwlp-host running there, and is this phone on the same Wi-Fi?"
        else -> "No PCs answered. Is rcwlp-host running on the PC, and is this phone on the same Wi-Fi?"
    }

    private fun render() {
        hostList.removeAllViews()
        val paired = store.hosts()
        val online = found.associateBy { it.idHex }
        for (h in paired) {
            val seen = online[h.idHex]
            val problem = seen?.incompatibility()
            val (badge, detail) = when {
                problem != null -> Badge("Paired") to problem
                seen != null -> Badge("Online", active = true) to "Tap to connect"
                else -> Badge("Paired") to "No answer at ${h.address} · tap to try anyway"
            }
            val onClick = if (problem != null) ({ toast(problem) }) else ({ startController(h) })
            addRow(h.name, badge, "$detail\nLong-press to forget", onClick) { confirmForget(h) }
        }
        for (f in found.filter { f -> paired.none { it.idHex == f.idHex } }) {
            val problem = f.incompatibility()
            if (problem == null) {
                addRow(f.name, Badge("New", active = true), "${f.address} · tap to pair", { startPairing(f) }, null)
            } else {
                addRow(f.name, Badge("New"), "${f.address} · $problem", { toast(problem) }, null)
            }
        }
    }

    /** The small label at the end of a PC's row; rose when there's something to tap for. */
    private data class Badge(val text: String, val active: Boolean = false)

    private fun addRow(title: String, badge: Badge, detail: String, onClick: () -> Unit, onLongClick: (() -> Unit)?) {
        val row = layoutInflater.inflate(R.layout.item_host, hostList, false)
        row.findViewById<TextView>(R.id.name).text = title
        row.findViewById<TextView>(R.id.detail).text = detail
        row.findViewById<TextView>(R.id.badge).apply {
            text = badge.text
            setBackgroundResource(if (badge.active) R.drawable.bg_badge_active else R.drawable.bg_badge)
            setTextColor(getColor(if (badge.active) R.color.rose else R.color.zinc_400))
        }
        row.setOnClickListener { onClick() }
        if (onLongClick != null) row.setOnLongClickListener { onLongClick(); true }
        hostList.addView(row)
    }

    /** A dialog laid out like the rest of the app: [body] gets the message, and the code or field when used. */
    private fun dialog(title: String, message: String): Pair<AlertDialog.Builder, View> {
        val body = layoutInflater.inflate(R.layout.dialog_body, null)
        body.findViewById<TextView>(R.id.message).text = message
        return AlertDialog.Builder(this).setTitle(title).setView(body) to body
    }

    /** Shows [builder]'s dialog with SimonSays's quiet grey for the button that backs out. */
    private fun show(builder: AlertDialog.Builder): AlertDialog =
        builder.show().also { it.getButton(AlertDialog.BUTTON_NEGATIVE)?.setTextColor(getColor(R.color.zinc_400)) }

    private fun startPairing(host: FoundHost) {
        val pairing = Pairing(store, host, deviceName())
        val (builder, body) = dialog("Pairing with ${host.name}", "Contacting the PC…")
        val dialog = show(
            builder
                .setNegativeButton("Cancel") { _, _ -> pairing.cancelled = true }
                .setCancelable(false),
        )
        thread {
            val result = pairing.run { code ->
                runOnUiThread {
                    body.findViewById<View>(R.id.code_label).visibility = View.VISIBLE
                    body.findViewById<TextView>(R.id.code).apply {
                        text = "%03d %03d".format(code / 1000, code % 1000)
                        visibility = View.VISIBLE
                    }
                    body.findViewById<TextView>(R.id.message).text =
                        "Type this code in the RCWLP window on your PC to finish pairing."
                }
            }
            runOnUiThread {
                if (isDestroyed) return@runOnUiThread
                dialog.dismiss()
                when (result) {
                    is Pairing.Result.Paired -> {
                        toast("Paired with ${host.name}")
                        startController(result.host)
                    }
                    Pairing.Result.Rejected -> toast("The PC rejected the pairing.")
                    Pairing.Result.TimedOut -> toast("Pairing timed out. Type the code on the PC within a minute.")
                    Pairing.Result.Cancelled -> {}
                    is Pairing.Result.Failed -> toast("Pairing failed: ${result.message}")
                }
                render()
            }
        }
    }

    private fun askForAddress() {
        val (builder, body) = dialog("Add PC by IP address", "The PC's local IP address, optionally with :port.")
        val input = body.findViewById<EditText>(R.id.input).apply {
            hint = "192.168.1.20"
            visibility = View.VISIBLE
        }
        show(
            builder
                .setPositiveButton("Add") { _, _ -> probe(input.text.toString().trim()) }
                .setNegativeButton("Cancel", null),
        )
        input.requestFocus()
    }

    private fun probe(text: String) {
        if (text.isEmpty()) return
        val match = Regex("^([^:]+):(\\d{1,5})$").find(text)
        val address = match?.groupValues?.get(1) ?: text
        val port = match?.groupValues?.get(2)?.toIntOrNull() ?: Proto.DEFAULT_PORT
        toast("Looking for a PC at $address…")
        thread {
            val host = runCatching { Discovery.probe(address, port) }.getOrNull()
            runOnUiThread {
                if (isDestroyed) return@runOnUiThread
                if (host == null) {
                    toast("No RCWLP host answered at $address. Is it running, and allowed through the Windows firewall?")
                    return@runOnUiThread
                }
                found = found.filter { it.idHex != host.idHex } + host
                render()
                host.incompatibility()?.let { return@runOnUiThread toast(it) }
                store.find(host.idHex)?.let(::startController) ?: startPairing(host)
            }
        }
    }

    private fun confirmForget(host: PairedHost) {
        val (builder, _) = dialog("Forget ${host.name}?", "You'll need to pair again to use it. The PC is asked to forget this phone too.")
        show(builder
            .setPositiveButton("Forget") { _, _ ->
                thread {
                    // Ask the PC first: the request is signed with the pair key, which goes away next.
                    val confirmed = runCatching { Unpairing.run(store.clientId, host) }.getOrDefault(false)
                    runOnUiThread {
                        store.remove(host.idHex)
                        if (isDestroyed) return@runOnUiThread
                        render()
                        toast(
                            if (confirmed) "Forgot ${host.name}, here and on the PC."
                            else "Forgot ${host.name} here. The PC didn't answer, so remove this phone there too: type list, then forget <n>.",
                        )
                    }
                }
            }
            .setNegativeButton("Cancel", null))
    }

    private fun startController(host: PairedHost) {
        if (store.backgroundMode && ShizukuState.current(this) != ShizukuState.READY) {
            toast("Background mode needs Shizuku first. See the steps under \"Keep working when locked\", or turn it off.")
            return
        }
        if (GamepadMapper.connected().isEmpty()) toast("No controller detected yet. Attach one and it will be picked up.")
        startActivity(Intent(this, ControllerActivity::class.java).putExtra(ControllerActivity.EXTRA_HOST_ID, host.idHex))
    }

    private fun deviceName(): String =
        Settings.Global.getString(contentResolver, Settings.Global.DEVICE_NAME) ?: "${Build.MANUFACTURER} ${Build.MODEL}"

    private fun toast(message: String) = Toast.makeText(this, message, Toast.LENGTH_LONG).show()
}
