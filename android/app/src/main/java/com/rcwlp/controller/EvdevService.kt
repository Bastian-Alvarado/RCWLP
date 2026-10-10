package com.rcwlp.controller

import android.content.Context
import android.os.FileObserver
import android.os.RemoteException
import android.util.Log
import android.view.KeyEvent
import android.view.MotionEvent
import androidx.annotation.Keep
import java.io.File
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import kotlin.math.abs
import kotlin.math.max
import kotlin.system.exitProcess

/**
 * Background mode's input reader. Shizuku runs this in a separate process with shell
 * privileges, which can open /dev/input. It grabs every USB or Bluetooth controller, so the phone
 * ignores it even when locked or in another app, and reports the merged state to the app.
 * When the app unbinds (or dies) Shizuku calls [destroy] and the process exits, which also
 * releases the grab and gives the controller back to Android.
 */
class EvdevService() : IEvdevService.Stub() {
    /** Shizuku v13+ prefers this constructor. */
    @Keep
    constructor(@Suppress("UNUSED_PARAMETER") context: Context) : this()

    private val lock = Any()
    private val pads = ConcurrentHashMap<String, EvdevPad>()
    @Volatile private var callback: IEvdevCallback? = null
    @Volatile private var running = false
    private var lastSent: PadState? = null
    private val rescan = ArrayBlockingQueue<Unit>(1)
    /** Built-in gamepad-like devices already logged as skipped. Scan thread only. */
    private val skipped = HashSet<String>()

    /** Wakes the scan thread when a device appears. Kept in a field: a collected observer stops watching. */
    @Suppress("DEPRECATION") // FileObserver(File, Int) needs API 29.
    private val hotplug = object : FileObserver(INPUT_DIR, CREATE or ATTRIB or DELETE) {
        override fun onEvent(event: Int, path: String?) {
            rescan.offer(Unit)
        }
    }

    override fun destroy() {
        running = false
        exitProcess(0)
    }

    override fun start(callback: IEvdevCallback) {
        this.callback = callback
        synchronized(lock) { lastSent = null }
        reportDevices()
        report()
        if (running) return
        running = true
        runCatching { hotplug.startWatching() }.onFailure { Log.w(TAG, "Can't watch $INPUT_DIR; polling only", it) }
        thread(name = "evdev-scan", isDaemon = true) {
            while (running) {
                runCatching { scan() }.onFailure { Log.w(TAG, "Scan failed", it) }
                // Hotplug wakes us right away. The timeout is a fallback for missed changes, like a
                // node that is still root-only when it appears and never gets an ATTRIB event.
                if (rescan.poll(SCAN_INTERVAL_MS, TimeUnit.MILLISECONDS) != null) {
                    Thread.sleep(SETTLE_MS) // A controller adds several nodes; scan once for all of them.
                    rescan.clear()
                }
            }
        }
    }

    /** Opens controllers that appeared since the last scan. */
    private fun scan() {
        val paths = File(INPUT_DIR).list().orEmpty()
            .filter { it.startsWith("event") }
            .map { "$INPUT_DIR/$it" }
            .filter { !pads.containsKey(it) }
        val found = paths.mapNotNull(::probe)
        // Only USB and Bluetooth devices, so built-in buttons (gaming phones' shoulder triggers)
        // and other apps' virtual gamepads keep working. Controllers often expose extra
        // interfaces with the same IDs, like a "Consumer Control" device carrying the home
        // button. Grab those too, or they'd still act on the phone.
        val controllerIds = (pads.values.map { it.vidPid } + found.filter { it.isGamepad && it.isExternal }.map { it.vidPid })
            .filter { it != 0 }
            .toSet()
        var added = false
        for (dev in found) {
            if (dev.isExternal && (dev.isGamepad || dev.vidPid in controllerIds)) {
                val pad = EvdevPad(dev)
                pads[dev.path] = pad
                thread(name = "evdev-${dev.path.substringAfterLast('/')}", isDaemon = true) { readLoop(pad) }
                added = true
            } else {
                if (dev.isGamepad && skipped.add("${dev.path} ${dev.name}")) {
                    Log.i(TAG, "Leaving ${dev.name} alone: it isn't on USB or Bluetooth (bus ${dev.id[0]})")
                }
                Evdev.close(dev.fd)
            }
        }
        if (added) reportDevices()
    }

    private fun probe(path: String): Probed? {
        val fd = Evdev.open(path)
        if (fd < 0) return null
        val id = Evdev.id(fd) ?: IntArray(4)
        return Probed(path, fd, Evdev.name(fd) ?: "", id, Evdev.keyBits(fd), Evdev.absBits(fd))
    }

    private fun readLoop(pad: EvdevPad) {
        val grab = Evdev.grab(pad.fd, true)
        if (grab != 0) Log.w(TAG, "Could not grab ${pad.name} (errno ${-grab}); the phone will see it too")
        Log.i(TAG, "Reading ${pad.name} at ${pad.path}")
        val buf = IntArray(64 * 3)
        try {
            while (running) {
                val n = Evdev.read(pad.fd, buf, 250)
                if (n < 0) break // Unplugged.
                if (n == 0) continue
                var frameDone = false
                synchronized(lock) {
                    for (i in 0 until n) {
                        if (pad.onEvent(buf[i * 3], buf[i * 3 + 1], buf[i * 3 + 2])) frameDone = true
                    }
                }
                if (frameDone) report()
            }
        } finally {
            pads.remove(pad.path)
            Evdev.grab(pad.fd, false)
            Evdev.close(pad.fd)
            Log.i(TAG, "Stopped reading ${pad.name}")
            reportDevices()
            report()
        }
    }

    /** Sends the merged state of all controllers if it changed. */
    private fun report() {
        val state = synchronized(lock) {
            val merged = merge(pads.values.map { it.state() })
            if (merged == lastSent) return
            lastSent = merged
            merged
        }
        try {
            callback?.onState(state.buttons, state.leftTrigger, state.rightTrigger, state.lx, state.ly, state.rx, state.ry)
        } catch (e: RemoteException) {
            Log.w(TAG, "App is gone", e)
        }
    }

    private fun reportDevices() {
        val names = pads.values.filter { it.isGamepad }.joinToString("\n") { it.name }
        try {
            callback?.onDevices(names)
        } catch (e: RemoteException) {
            Log.w(TAG, "App is gone", e)
        }
    }

    private fun merge(states: List<PadState>): PadState {
        fun strongest(pick: (PadState) -> Int) = states.map(pick).maxByOrNull { abs(it) } ?: 0
        return PadState(
            buttons = states.fold(0) { acc, s -> acc or s.buttons },
            leftTrigger = states.maxOfOrNull { it.leftTrigger } ?: 0,
            rightTrigger = states.maxOfOrNull { it.rightTrigger } ?: 0,
            lx = strongest { it.lx },
            ly = strongest { it.ly },
            rx = strongest { it.rx },
            ry = strongest { it.ry },
        )
    }

    private companion object {
        const val TAG = "RCWLP-evdev"
        const val INPUT_DIR = "/dev/input"
        /** Fallback when hotplug events are missed. */
        const val SCAN_INTERVAL_MS = 5000L
        const val SETTLE_MS = 250L
    }
}

/** An input device we opened to look at. */
class Probed(
    val path: String,
    val fd: Int,
    val name: String,
    /** [bustype, vendor, product, version] */
    val id: IntArray,
    val keyBits: ByteArray?,
    val absBits: ByteArray?,
) {
    val isGamepad = Evdev.GAMEPAD_BUTTONS.any { Evdev.hasBit(keyBits, it) }
    val isExternal = id[0] == BUS_USB || id[0] == BUS_BLUETOOTH
    val vidPid = (id[1] shl 16) or id[2]

    private companion object {
        const val BUS_USB = 0x03
        const val BUS_BLUETOOTH = 0x05
    }
}

/**
 * One grabbed device, translated the way Android's input stack would: scan codes and axes go
 * through the device's key layout, axes are normalized like JoystickInputMapper does, and the
 * result is mapped to XInput exactly as [GamepadMapper] does in foreground mode.
 */
class EvdevPad(dev: Probed) {
    val path = dev.path
    val fd = dev.fd
    val name = dev.name
    val vidPid = dev.vidPid
    val isGamepad = dev.isGamepad

    private class Range(val min: Int, val max: Int, val flat: Int)

    private val layout = KeyLayout.load(dev.id[1], dev.id[2], dev.id[3], dev.name)
    private val ranges = HashMap<Int, Range>()
    private val axes = HashMap<Int, Float>()
    private var keyButtons = 0
    private var digitalLt = false
    private var digitalRt = false
    private var dropping = false

    init {
        for ((code, axis) in layout.axes) {
            if (!Evdev.hasBit(dev.absBits, code)) continue
            val info = Evdev.absInfo(fd, code) ?: continue
            ranges[code] = Range(info[1], info[2], axis.flat ?: info[4])
            axes[axis.axis] = 0f
            if (axis.splitValue != null) axes[axis.highAxis] = 0f
            onAbs(code, info[0])
        }
    }

    /** Returns true at the end of an event frame (SYN_REPORT). */
    fun onEvent(type: Int, code: Int, value: Int): Boolean {
        if (type == Evdev.EV_SYN) {
            when (code) {
                Evdev.SYN_DROPPED -> dropping = true
                Evdev.SYN_REPORT -> {
                    if (dropping) resync()
                    dropping = false
                    return true
                }
            }
            return false
        }
        if (dropping) return false // The kernel's buffer overflowed; skip to the next frame.
        when (type) {
            Evdev.EV_KEY -> onKey(code, value != 0)
            Evdev.EV_ABS -> onAbs(code, value)
        }
        return false
    }

    private fun onKey(code: Int, down: Boolean) {
        when (val keyCode = layout.keys[code] ?: return) {
            KeyEvent.KEYCODE_BUTTON_L2 -> digitalLt = down
            KeyEvent.KEYCODE_BUTTON_R2 -> digitalRt = down
            else -> {
                val bit = GamepadMapper.buttonFor(keyCode)
                if (bit != 0) keyButtons = if (down) keyButtons or bit else keyButtons and bit.inv()
            }
        }
    }

    private fun onAbs(code: Int, raw: Int) {
        val axis = layout.axes[code] ?: return
        val r = ranges[code] ?: return
        val split = axis.splitValue
        if (split != null) {
            axes[axis.axis] = if (raw < split && split > r.min) (split - raw).toFloat() / (split - r.min) else 0f
            axes[axis.highAxis] = if (raw > split && r.max > split) (raw - split).toFloat() / (r.max - split) else 0f
            return
        }
        val span = (r.max - r.min).toFloat()
        if (span <= 0f) return
        var v: Float
        val flat: Float
        if (axis.axis in CENTERED) {
            v = 2f * (raw - r.min) / span - 1f
            flat = 2f * r.flat / span
        } else {
            v = (raw - r.min) / span
            flat = r.flat / span
        }
        if (axis.invert) v = -v
        axes[axis.axis] = if (abs(v) <= flat) 0f else v
    }

    /** After dropped events: re-read axis positions and release keys rather than guess. */
    private fun resync() {
        keyButtons = 0
        digitalLt = false
        digitalRt = false
        for (code in ranges.keys) Evdev.absInfo(fd, code)?.let { onAbs(code, it[0]) }
    }

    fun state(): PadState {
        fun a(axis: Int) = axes[axis] ?: 0f
        // Same choices as GamepadMapper.onMotion.
        val rxAxis = if (MotionEvent.AXIS_Z in axes) MotionEvent.AXIS_Z else MotionEvent.AXIS_RX
        val ryAxis = if (MotionEvent.AXIS_RZ in axes) MotionEvent.AXIS_RZ else MotionEvent.AXIS_RY
        val hx = a(MotionEvent.AXIS_HAT_X)
        val hy = a(MotionEvent.AXIS_HAT_Y)
        val hat = (if (hx < -0.5f) XButton.LEFT else 0) or
            (if (hx > 0.5f) XButton.RIGHT else 0) or
            (if (hy < -0.5f) XButton.UP else 0) or
            (if (hy > 0.5f) XButton.DOWN else 0)
        return PadState(
            buttons = keyButtons or hat,
            leftTrigger = if (digitalLt) 255 else GamepadMapper.trigger(max(a(MotionEvent.AXIS_LTRIGGER), a(MotionEvent.AXIS_BRAKE))),
            rightTrigger = if (digitalRt) 255 else GamepadMapper.trigger(max(a(MotionEvent.AXIS_RTRIGGER), a(MotionEvent.AXIS_GAS))),
            lx = GamepadMapper.stick(a(MotionEvent.AXIS_X)),
            ly = GamepadMapper.stick(-a(MotionEvent.AXIS_Y)),
            rx = GamepadMapper.stick(a(rxAxis)),
            ry = GamepadMapper.stick(-a(ryAxis)),
        )
    }

    private companion object {
        /** Axes Android scales to -1..1 (JoystickInputMapper); the rest go 0..1. */
        val CENTERED = setOf(
            MotionEvent.AXIS_X, MotionEvent.AXIS_Y, MotionEvent.AXIS_Z,
            MotionEvent.AXIS_RX, MotionEvent.AXIS_RY, MotionEvent.AXIS_RZ,
            MotionEvent.AXIS_HAT_X, MotionEvent.AXIS_HAT_Y,
            MotionEvent.AXIS_ORIENTATION, MotionEvent.AXIS_RUDDER, MotionEvent.AXIS_WHEEL,
        )
    }
}
