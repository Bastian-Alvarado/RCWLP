package com.rcwlp.controller

import android.app.Activity
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Rect
import android.hardware.input.InputManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.Surface
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.widget.TextView

/**
 * The phone-as-controller screen: black, dims itself, and forwards every controller event to the PC.
 * The session runs only while this screen is visible, since Android stops delivering controller
 * input to apps that are in the background or behind the lock screen.
 *
 * Each attached controller gets its own slot, and so its own virtual controller on the PC. In
 * PS4 mode the screen is the DualShock 4 touchpad, and a quick tap clicks it.
 */
class ControllerActivity : Activity(), ControllerSession.Listener, InputManager.InputDeviceListener {
    /** Controllers by Android input device id. */
    private val slots = SlotAssigner<Int>()
    private val mappers = HashMap<Int, GamepadMapper>()
    /** Rumble per slot. Only slot 0, the controller clipped to the phone, may buzz the phone itself. */
    private val rumblers = java.util.concurrent.ConcurrentHashMap<Int, Rumbler>()
    private val touchpad = Touchpad()
    private val handler = Handler(Looper.getMainLooper())
    private val dim = Runnable { setDimmed(true) }
    private val refreshStatus = object : Runnable {
        override fun run() {
            showStatus()
            handler.postDelayed(this, STATUS_REFRESH_MS)
        }
    }
    private val releaseClick = Runnable { clickHeld = false; pushState(0) }
    private lateinit var store: HostStore
    private lateinit var host: PairedHost
    private lateinit var content: View
    private lateinit var lightbar: View
    private lateinit var statusText: TextView
    private lateinit var controllerText: TextView
    private lateinit var inputText: TextView
    private lateinit var disconnectButton: View
    private var session: ControllerSession? = null
    private var motion: MotionSource? = null
    private var ps4Mode = false
    /** Background mode: [ControllerService] runs the connection instead of this screen. */
    private var background = false
    private var shownInput: PadState? = null
    private var clickHeld = false
    /** The session's state message, before the latency is added to it. */
    private var stateText = ""
    private var connected = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        store = HostStore(this)
        host = store.find(intent.getStringExtra(EXTRA_HOST_ID).orEmpty()) ?: run {
            finish()
            return
        }
        setContentView(R.layout.activity_controller)
        content = findViewById(R.id.content)
        lightbar = findViewById(R.id.lightbar)
        statusText = findViewById(R.id.status)
        controllerText = findViewById(R.id.controller)
        inputText = findViewById(R.id.input)
        disconnectButton = findViewById(R.id.disconnect)
        findViewById<TextView>(R.id.host_name).text = host.name
        disconnectButton.setOnClickListener {
            if (background) ControllerService.stop(this)
            finish()
        }
    }

    override fun onStart() {
        super.onStart()
        background = store.backgroundMode
        if (background) {
            // The service owns the connection; this screen only shows its status and can be left.
            window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            findViewById<TextView>(R.id.hint).setText(R.string.background_hint)
            ControllerService.start(this, host.idHex)
            ControllerService.observe(serviceObserver)
            wake()
            return
        }
        // A background-mode connection still running would grab the controller away from this
        // screen and fight this session for the PC.
        ControllerService.stop(this)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        ps4Mode = store.ps4Mode
        findViewById<TextView>(R.id.hint).setText(if (ps4Mode) R.string.controller_hint_ps4 else R.string.controller_hint)
        val padType = if (ps4Mode) Proto.PAD_DUALSHOCK4 else Proto.PAD_XBOX360
        // Slot 0 can always fall back to the phone's own vibration, even with no controller attached.
        rumblers[0] = Rumbler(this)
        val s = ControllerSession(store, host, padType, this, { batteryLevel(this) }).also { it.start() }
        session = s
        if (ps4Mode) {
            motion = MotionSource(this) { s.update(it) }.also {
                it.rotation = displayRotation()
                it.start()
            }
        }
        getSystemService(InputManager::class.java).registerInputDeviceListener(this, handler)
        // Controllers already attached get slots in the order Android found them.
        GamepadMapper.connected().forEach { attach(it.id) }
        updateControllers()
        handler.post(refreshStatus)
        wake()
    }

    override fun onStop() {
        super.onStop()
        handler.removeCallbacks(dim)
        handler.removeCallbacks(refreshStatus)
        handler.removeCallbacks(releaseClick)
        if (background) {
            ControllerService.unobserve(serviceObserver)
            return
        }
        motion?.stop()
        motion = null
        session?.stop()
        session = null
        rumblers.values.forEach(Rumbler::stop)
        rumblers.clear()
        mappers.clear()
        slots.assigned().forEach { (_, id) -> slots.release(id) }
        getSystemService(InputManager::class.java).unregisterInputDeviceListener(this)
    }

    private val serviceObserver: (ControllerService.Status) -> Unit = { s ->
        runOnUiThread {
            if (s.message.isNotEmpty()) statusText.text = s.message
            controllerText.text = s.controllers.lineSequence().filter { it.isNotBlank() }.toList()
                .takeIf { it.isNotEmpty() }
                ?.joinToString("\n") { "Controller: $it" }
                ?: if (s.active) "Waiting for a controller…" else ""
        }
    }

    // The manifest handles orientation changes itself, so the motion axes must follow here.
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        motion?.rotation = displayRotation()
    }

    private fun displayRotation(): Int =
        if (Build.VERSION.SDK_INT >= 30) {
            display?.rotation ?: Surface.ROTATION_90
        } else {
            @Suppress("DEPRECATION")
            windowManager.defaultDisplay.rotation
        }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) hideSystemBars()
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (background || !GamepadMapper.isFromController(event)) return super.dispatchKeyEvent(event)
        // Consume every controller key, so B or Back never navigate away, even from a fifth controller.
        val slot = attach(event.deviceId) ?: return true
        mapper(event.deviceId).onKey(event)
        pushState(slot)
        return true
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean {
        if (!background) {
            val slot = if (GamepadMapper.isFromController(event)) attach(event.deviceId) else null
            if (slot != null && mapper(event.deviceId).onMotion(event)) {
                pushState(slot)
                return true
            }
        }
        return super.dispatchGenericMotionEvent(event)
    }

    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        if (event.actionMasked == MotionEvent.ACTION_DOWN) wake()
        if (ps4Mode && !background && !onDisconnectButton(event)) {
            onTouchpad(event)
            return true
        }
        return super.dispatchTouchEvent(event)
    }

    private fun onDisconnectButton(event: MotionEvent): Boolean {
        if (disconnectButton.visibility != View.VISIBLE) return false
        val r = Rect()
        disconnectButton.getGlobalVisibleRect(r)
        return r.contains(event.getX(event.actionIndex).toInt(), event.getY(event.actionIndex).toInt())
    }

    private fun onTouchpad(event: MotionEvent) {
        val lifting = event.actionMasked == MotionEvent.ACTION_UP ||
            event.actionMasked == MotionEvent.ACTION_POINTER_UP ||
            event.actionMasked == MotionEvent.ACTION_CANCEL
        val pointers = (0 until event.pointerCount)
            .filterNot { lifting && (it == event.actionIndex || event.actionMasked == MotionEvent.ACTION_CANCEL) }
            .map { Triple(event.getPointerId(it), event.getX(it), event.getY(it)) }
        val root = window.decorView
        val tapped = touchpad.update(pointers, root.width, root.height, SystemClock.uptimeMillis())
        session?.update(touchpad.touch)
        if (tapped) {
            clickHeld = true
            pushState(0)
            handler.removeCallbacks(releaseClick)
            handler.postDelayed(releaseClick, Touchpad.CLICK_MS)
        }
    }

    private fun mapper(deviceId: Int) = mappers.getOrPut(deviceId) { GamepadMapper() }

    /** Gives a controller its slot (and the PC a virtual controller for it). Null when all are taken. */
    private fun attach(deviceId: Int): Int? {
        slots.slotOf(deviceId)?.let { return it }
        val slot = slots.assign(deviceId) ?: return null
        rumblers[slot] = Rumbler(this, phoneFallback = slot == 0).also { it.device = InputDevice.getDevice(deviceId) }
        pushState(slot)
        updateControllers()
        return slot
    }

    private fun pushState(slot: Int) {
        val deviceId = slots.assigned().firstOrNull { it.first == slot }?.second
        var state = deviceId?.let { mappers[it]?.state() } ?: PadState()
        if (slot == 0 && clickHeld) state = state.copy(buttons = state.buttons or XButton.TOUCHPAD_CLICK)
        session?.update(slot, state)
        if (state != shownInput && slot == 0) {
            shownInput = state
            inputText.text = describe(state)
        }
    }

    override fun onState(state: ControllerSession.State, detail: Int) = runOnUiThread {
        connected = state == ControllerSession.State.CONNECTED
        stateText = ControllerSession.describe(state, detail, host, ps4Mode)
        if (!connected) lightbar.visibility = View.GONE
        showStatus()
    }

    /** The state message, plus the round trip while connected. */
    private fun showStatus() {
        val latency = session?.latencyMs?.takeIf { connected }
        statusText.text = if (latency != null) "$stateText · $latency ms" else stateText
    }

    override fun onFeedback(pads: List<Feedback>) {
        if (pads.isEmpty()) {
            rumblers.values.forEach(Rumbler::stop)
            return
        }
        for (f in pads) rumblers[f.slot]?.rumble(f.large, f.small)
        val color = pads.firstOrNull { it.slot == 0 }?.lightbar ?: 0
        runOnUiThread {
            if (color == 0) {
                lightbar.visibility = View.GONE
            } else {
                lightbar.setBackgroundColor(Color.rgb(color shr 16 and 0xff, color shr 8 and 0xff, color and 0xff))
                lightbar.visibility = View.VISIBLE
            }
        }
    }

    override fun onInputDeviceAdded(deviceId: Int) {
        if (InputDevice.getDevice(deviceId)?.let { GamepadMapper.isController(it) } == true) attach(deviceId)
        updateControllers()
    }

    override fun onInputDeviceRemoved(deviceId: Int) {
        val slot = slots.release(deviceId)
        mappers.remove(deviceId)
        if (slot != null) {
            rumblers.remove(slot)?.stop()
            session?.remove(slot)
        }
        updateControllers()
    }

    override fun onInputDeviceChanged(deviceId: Int) = updateControllers()

    private fun updateControllers() {
        val attached = slots.assigned()
        controllerText.text = if (attached.isEmpty()) {
            "No controller detected"
        } else {
            attached.joinToString("\n") { (slot, id) ->
                val name = InputDevice.getDevice(id)?.name ?: "Controller"
                if (attached.size == 1) "Controller: $name" else "Controller ${slot + 1}: $name"
            }
        }
    }

    /** Full brightness on touch, then dim to save battery while playing. */
    private fun wake() {
        setDimmed(false)
        handler.removeCallbacks(dim)
        handler.postDelayed(dim, DIM_AFTER_MS)
    }

    private fun setDimmed(dimmed: Boolean) {
        window.attributes = window.attributes.apply {
            screenBrightness = if (dimmed) 0f else WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE
        }
        content.alpha = if (dimmed) 0.25f else 1f
        disconnectButton.visibility = if (dimmed) View.INVISIBLE else View.VISIBLE
    }

    private fun hideSystemBars() {
        if (Build.VERSION.SDK_INT >= 30) {
            window.insetsController?.let {
                it.hide(WindowInsets.Type.systemBars())
                it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            }
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or
                View.SYSTEM_UI_FLAG_FULLSCREEN or
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
                View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or
                View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
        }
    }

    private fun describe(s: PadState): String {
        val parts = XButton.NAMES.filter { (bit, _) -> s.buttons and bit != 0 }.map { it.second }.toMutableList()
        if (s.leftTrigger > 30) parts += "LT"
        if (s.rightTrigger > 30) parts += "RT"
        if (kotlin.math.hypot(s.lx.toDouble(), s.ly.toDouble()) > 8000) parts += "L-stick"
        if (kotlin.math.hypot(s.rx.toDouble(), s.ry.toDouble()) > 8000) parts += "R-stick"
        return if (parts.isEmpty()) " " else parts.joinToString("  ")
    }

    companion object {
        const val EXTRA_HOST_ID = "host_id"
        private const val DIM_AFTER_MS = 10_000L
        private const val STATUS_REFRESH_MS = 1_000L
    }
}
