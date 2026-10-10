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
import android.text.InputType
import android.util.TypedValue
import android.view.View
import android.view.ViewGroup
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
    private lateinit var controllerLabel: TextView
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
        controllerLabel = findViewById(R.id.controller_label)
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
        deadzoneSlider(R.id.stick_deadzone, R.id.stick_deadzone_label, "Stick dead zone", store.stickDeadzone) { store.stickDeadzone = it }
        deadzoneSlider(R.id.trigger_deadzone, R.id.trigger_deadzone_label, "Trigger dead zone", store.triggerDeadzone) { store.triggerDeadzone = it }
        Shizuku.addBinderReceivedListenerSticky(shizukuChanged)
        Shizuku.addBinderDeadListener(shizukuChanged)
        Shizuku.addRequestPermissionResultListener(shizukuPermissionResult)
    }

    /** A 0–30% slider that saves as it moves; applies from the next connection. */
    private fun deadzoneSlider(sliderId: Int, labelId: Int, title: String, value: Int, save: (Int) -> Unit) {
        val label = findViewById<TextView>(labelId)
        fun show(v: Int) {
            label.text = if (v == 0) "$title: off" else "$title: $v%"
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
        val controller = GamepadMapper.connected().firstOrNull()
        controllerLabel.text = controller?.let { "Controller: ${it.name}" } ?: "No controller detected. Attach one to use this phone as a gamepad."
    }

    private fun scan() {
        if (scanning) return
        scanning = true
        scanStatus.text = "Searching for PCs…"
        thread {
            val result = runCatching { Discovery.scan(store.hosts()) }.getOrDefault(emptyList())
            runOnUiThread {
                scanning = false
                if (isDestroyed) return@runOnUiThread
                found = result
                scanStatus.text = if (result.isEmpty()) noPcsMessage(store.hosts()) else ""
                render()
            }
        }
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
            val detail = when {
                problem != null -> "Paired · $problem"
                seen != null -> "Paired · online · tap to connect"
                else -> "Paired · no answer at ${h.address} · tap to try anyway"
            }
            val onClick = if (problem != null) ({ toast(problem) }) else ({ startController(h) })
            addRow(h.name, "$detail\nLong-press to forget", onClick) { confirmForget(h) }
        }
        for (f in found.filter { f -> paired.none { it.idHex == f.idHex } }) {
            val problem = f.incompatibility()
            if (problem == null) {
                addRow(f.name, "${f.address} · tap to pair", { startPairing(f) }, null)
            } else {
                addRow(f.name, "${f.address} · $problem", { toast(problem) }, null)
            }
        }
    }

    private fun addRow(title: String, detail: String, onClick: () -> Unit, onLongClick: (() -> Unit)?) {
        val pad = dp(16)
        val row = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(pad, pad, pad, pad)
            isClickable = true
            val ripple = TypedValue()
            theme.resolveAttribute(android.R.attr.selectableItemBackground, ripple, true)
            setBackgroundResource(ripple.resourceId)
            setOnClickListener { onClick() }
            if (onLongClick != null) setOnLongClickListener { onLongClick(); true }
        }
        row.addView(TextView(this).apply {
            text = title
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 20f)
        })
        row.addView(TextView(this).apply {
            text = detail
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 14f)
            alpha = 0.7f
        })
        hostList.addView(row, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
    }

    private fun startPairing(host: FoundHost) {
        val pairing = Pairing(store, host, deviceName())
        val dialog = AlertDialog.Builder(this)
            .setTitle("Pairing with ${host.name}")
            .setMessage("Contacting the PC…")
            .setNegativeButton("Cancel") { _, _ -> pairing.cancelled = true }
            .setCancelable(false)
            .show()
        thread {
            val result = pairing.run { code ->
                runOnUiThread {
                    dialog.setMessage(
                        "Code: %03d %03d\n\nType this code in the RCWLP host window on your PC to finish pairing."
                            .format(code / 1000, code % 1000),
                    )
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
        val input = EditText(this).apply {
            hint = "192.168.1.20"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
        }
        AlertDialog.Builder(this)
            .setTitle("Add PC by IP address")
            .setMessage("The PC's local IP address, optionally with :port.")
            .setView(input)
            .setPositiveButton("Add") { _, _ -> probe(input.text.toString().trim()) }
            .setNegativeButton("Cancel", null)
            .show()
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
        AlertDialog.Builder(this)
            .setTitle("Forget ${host.name}?")
            .setMessage("You'll need to pair again to use it. The PC is asked to forget this phone too.")
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
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun startController(host: PairedHost) {
        if (store.backgroundMode && ShizukuState.current(this) != ShizukuState.READY) {
            toast("Background mode needs Shizuku first. See the steps under the checkbox, or turn background mode off.")
            return
        }
        if (GamepadMapper.connected().isEmpty()) toast("No controller detected yet. Attach one and it will be picked up.")
        startActivity(Intent(this, ControllerActivity::class.java).putExtra(ControllerActivity.EXTRA_HOST_ID, host.idHex))
    }

    private fun deviceName(): String =
        Settings.Global.getString(contentResolver, Settings.Global.DEVICE_NAME) ?: "${Build.MANUFACTURER} ${Build.MODEL}"

    private fun toast(message: String) = Toast.makeText(this, message, Toast.LENGTH_LONG).show()

    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()
}
