package com.rcwlp.controller

import android.app.Activity
import android.content.res.Configuration
import android.hardware.input.InputManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
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
 */
class ControllerActivity : Activity(), ControllerSession.Listener, InputManager.InputDeviceListener {
    private val mapper = GamepadMapper()
    private val handler = Handler(Looper.getMainLooper())
    private val dim = Runnable { setDimmed(true) }
    private lateinit var store: HostStore
    private lateinit var host: PairedHost
    private lateinit var rumbler: Rumbler
    private lateinit var content: View
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

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        store = HostStore(this)
        host = store.find(intent.getStringExtra(EXTRA_HOST_ID).orEmpty()) ?: run {
            finish()
            return
        }
        setContentView(R.layout.activity_controller)
        rumbler = Rumbler(this)
        content = findViewById(R.id.content)
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
        findViewById<TextView>(R.id.hint).setText(R.string.controller_hint)
        mapper.reset()
        ps4Mode = store.ps4Mode
        val padType = if (ps4Mode) Proto.PAD_DUALSHOCK4 else Proto.PAD_XBOX360
        val s = ControllerSession(store, host, padType, this).also { it.start() }
        session = s
        if (ps4Mode) {
            motion = MotionSource(this) { s.update(it) }.also {
                it.rotation = displayRotation()
                it.start()
            }
        }
        getSystemService(InputManager::class.java).registerInputDeviceListener(this, handler)
        updateController()
        wake()
    }

    override fun onStop() {
        super.onStop()
        handler.removeCallbacks(dim)
        if (background) {
            ControllerService.unobserve(serviceObserver)
            return
        }
        motion?.stop()
        motion = null
        session?.stop()
        session = null
        rumbler.stop()
        getSystemService(InputManager::class.java).unregisterInputDeviceListener(this)
    }

    private val serviceObserver: (ControllerService.Status) -> Unit = { s ->
        runOnUiThread {
            if (s.message.isNotEmpty()) statusText.text = s.message
            controllerText.text = s.controllers.lineSequence().firstOrNull { it.isNotBlank() }
                ?.let { "Controller: $it" }
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
        if (mapper.onKey(event)) {
            pushState()
            return true
        }
        return super.dispatchKeyEvent(event)
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean {
        if (mapper.onMotion(event)) {
            pushState()
            return true
        }
        return super.dispatchGenericMotionEvent(event)
    }

    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        if (event.actionMasked == MotionEvent.ACTION_DOWN) wake()
        return super.dispatchTouchEvent(event)
    }

    private fun pushState() {
        val state = mapper.state()
        session?.update(state)
        if (state != shownInput) {
            shownInput = state
            inputText.text = describe(state)
        }
    }

    override fun onState(state: ControllerSession.State, player: Int) = runOnUiThread {
        statusText.text = when (state) {
            ControllerSession.State.CONNECTING -> "Connecting to PC…"
            ControllerSession.State.CONNECTED -> when {
                ps4Mode -> "Connected · PS4 controller with gyro"
                player > 0 -> "Connected · Player $player"
                else -> "Connected"
            }
            ControllerSession.State.NOT_PAIRED -> "This PC doesn't recognize this phone anymore.\nGo back and pair again."
        }
    }

    override fun onRumble(large: Int, small: Int) = rumbler.rumble(large, small)

    override fun onInputDeviceAdded(deviceId: Int) = updateController()
    override fun onInputDeviceRemoved(deviceId: Int) = updateController()
    override fun onInputDeviceChanged(deviceId: Int) = updateController()

    private fun updateController() {
        val device = GamepadMapper.connected().firstOrNull()
        rumbler.device = device
        controllerText.text = device?.let { "Controller: ${it.name}" } ?: "No controller detected"
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
    }
}
