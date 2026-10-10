package com.rcwlp.controller

import android.view.InputDevice
import android.view.InputEvent
import android.view.KeyEvent
import android.view.MotionEvent
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.roundToInt

/** One controller snapshot in XInput terms (what the PC's virtual Xbox 360 pad expects). */
data class PadState(
    val buttons: Int = 0,
    val leftTrigger: Int = 0,
    val rightTrigger: Int = 0,
    val lx: Int = 0,
    val ly: Int = 0,
    val rx: Int = 0,
    val ry: Int = 0,
)

object XButton {
    const val UP = 0x0001
    const val DOWN = 0x0002
    const val LEFT = 0x0004
    const val RIGHT = 0x0008
    const val START = 0x0010
    const val BACK = 0x0020
    const val LTHUMB = 0x0040
    const val RTHUMB = 0x0080
    const val LB = 0x0100
    const val RB = 0x0200
    const val GUIDE = 0x0400
    /** Unused by XInput; the protocol uses it for the DualShock 4 touchpad click. */
    const val TOUCHPAD_CLICK = 0x0800
    const val A = 0x1000
    const val B = 0x2000
    const val X = 0x4000
    const val Y = 0x8000

    val NAMES = listOf(
        A to "A", B to "B", X to "X", Y to "Y", LB to "LB", RB to "RB", LTHUMB to "LS", RTHUMB to "RS",
        BACK to "Back", START to "Start", GUIDE to "Guide", TOUCHPAD_CLICK to "Touchpad", UP to "Up", DOWN to "Down", LEFT to "Left", RIGHT to "Right",
    )
}

/** Turns Android controller events into a [PadState]. Events from any attached controller are merged. */
class GamepadMapper {
    private var keyButtons = 0
    private var hatButtons = 0
    private var digitalLt = false
    private var digitalRt = false
    private var analogLt = 0
    private var analogRt = 0
    private var lx = 0
    private var ly = 0
    private var rx = 0
    private var ry = 0

    fun state() = PadState(
        buttons = keyButtons or hatButtons,
        leftTrigger = if (digitalLt) 255 else analogLt,
        rightTrigger = if (digitalRt) 255 else analogRt,
        lx = lx, ly = ly, rx = rx, ry = ry,
    )

    fun reset() {
        keyButtons = 0; hatButtons = 0; digitalLt = false; digitalRt = false
        analogLt = 0; analogRt = 0; lx = 0; ly = 0; rx = 0; ry = 0
    }

    /** Returns true if the event came from a controller and was consumed. */
    fun onKey(event: KeyEvent): Boolean {
        if (!isFromController(event)) return false
        val down = event.action == KeyEvent.ACTION_DOWN
        when (event.keyCode) {
            KeyEvent.KEYCODE_BUTTON_L2 -> digitalLt = down
            KeyEvent.KEYCODE_BUTTON_R2 -> digitalRt = down
            else -> {
                val bit = buttonFor(event.keyCode)
                if (bit != 0) keyButtons = if (down) keyButtons or bit else keyButtons and bit.inv()
            }
        }
        // Consume every controller key, so B or Back never navigate away.
        return true
    }

    /** Returns true if the event came from a controller and was consumed. */
    fun onMotion(event: MotionEvent): Boolean {
        if (event.source and InputDevice.SOURCE_JOYSTICK != InputDevice.SOURCE_JOYSTICK) return false
        if (event.action != MotionEvent.ACTION_MOVE) return false
        val device = event.device

        lx = stick(axis(event, device, MotionEvent.AXIS_X))
        ly = stick(-axis(event, device, MotionEvent.AXIS_Y))
        // Most controllers put the right stick on Z/RZ; some older mappings use RX/RY.
        val rxAxis = if (device?.getMotionRange(MotionEvent.AXIS_Z) != null) MotionEvent.AXIS_Z else MotionEvent.AXIS_RX
        val ryAxis = if (device?.getMotionRange(MotionEvent.AXIS_RZ) != null) MotionEvent.AXIS_RZ else MotionEvent.AXIS_RY
        rx = stick(axis(event, device, rxAxis))
        ry = stick(-axis(event, device, ryAxis))
        // Controllers report triggers on LTRIGGER/RTRIGGER, BRAKE/GAS, or both.
        analogLt = trigger(max(axis(event, device, MotionEvent.AXIS_LTRIGGER), axis(event, device, MotionEvent.AXIS_BRAKE)))
        analogRt = trigger(max(axis(event, device, MotionEvent.AXIS_RTRIGGER), axis(event, device, MotionEvent.AXIS_GAS)))

        val hx = event.getAxisValue(MotionEvent.AXIS_HAT_X)
        val hy = event.getAxisValue(MotionEvent.AXIS_HAT_Y)
        hatButtons = (if (hx < -0.5f) XButton.LEFT else 0) or
            (if (hx > 0.5f) XButton.RIGHT else 0) or
            (if (hy < -0.5f) XButton.UP else 0) or
            (if (hy > 0.5f) XButton.DOWN else 0)
        return true
    }

    private fun axis(event: MotionEvent, device: InputDevice?, axis: Int): Float {
        val v = event.getAxisValue(axis)
        val flat = device?.getMotionRange(axis, event.source)?.flat ?: 0f
        return if (abs(v) <= flat) 0f else v
    }

    companion object {
        fun stick(v: Float) = (v.coerceIn(-1f, 1f) * 32767f).roundToInt()

        fun trigger(v: Float) = (v.coerceIn(0f, 1f) * 255f).roundToInt()

        /** XInput button bit for an Android keycode, or 0. Shared with background mode. */
        fun buttonFor(keyCode: Int) = when (keyCode) {
            KeyEvent.KEYCODE_BUTTON_A -> XButton.A
            KeyEvent.KEYCODE_BUTTON_B -> XButton.B
            KeyEvent.KEYCODE_BUTTON_X -> XButton.X
            KeyEvent.KEYCODE_BUTTON_Y -> XButton.Y
            KeyEvent.KEYCODE_BUTTON_L1 -> XButton.LB
            KeyEvent.KEYCODE_BUTTON_R1 -> XButton.RB
            KeyEvent.KEYCODE_BUTTON_THUMBL -> XButton.LTHUMB
            KeyEvent.KEYCODE_BUTTON_THUMBR -> XButton.RTHUMB
            KeyEvent.KEYCODE_BUTTON_START, KeyEvent.KEYCODE_MENU -> XButton.START
            KeyEvent.KEYCODE_BUTTON_SELECT, KeyEvent.KEYCODE_BACK -> XButton.BACK
            // HOME only arrives in background mode, where the controller is grabbed before Android sees it.
            KeyEvent.KEYCODE_BUTTON_MODE, KeyEvent.KEYCODE_HOME -> XButton.GUIDE
            KeyEvent.KEYCODE_DPAD_UP -> XButton.UP
            KeyEvent.KEYCODE_DPAD_DOWN -> XButton.DOWN
            KeyEvent.KEYCODE_DPAD_LEFT -> XButton.LEFT
            KeyEvent.KEYCODE_DPAD_RIGHT -> XButton.RIGHT
            else -> 0
        }

        fun isController(device: InputDevice): Boolean {
            if (device.isVirtual) return false
            val s = device.sources
            return s and InputDevice.SOURCE_GAMEPAD == InputDevice.SOURCE_GAMEPAD ||
                s and InputDevice.SOURCE_JOYSTICK == InputDevice.SOURCE_JOYSTICK
        }

        fun isFromController(event: InputEvent): Boolean {
            val s = event.source
            if (s and InputDevice.SOURCE_GAMEPAD == InputDevice.SOURCE_GAMEPAD ||
                s and InputDevice.SOURCE_JOYSTICK == InputDevice.SOURCE_JOYSTICK
            ) return true
            return event.device?.let(::isController) ?: false
        }

        fun connected(): List<InputDevice> =
            InputDevice.getDeviceIds().toList().mapNotNull(InputDevice::getDevice).filter(::isController)
    }
}
