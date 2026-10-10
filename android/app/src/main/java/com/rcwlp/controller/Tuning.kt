package com.rcwlp.controller

import kotlin.math.hypot
import kotlin.math.roundToInt

/**
 * Dead zones on top of the ones Android already applies, for worn sticks that drift or triggers
 * that never quite return to zero. Values in the dead zone read as rest, and the rest of the
 * range is stretched so full deflection still reaches the maximum.
 */
data class Tuning(
    /** Fraction of a stick's travel, from the centre, that reads as centred. */
    val stickDeadzone: Float = 0f,
    /** Fraction of a trigger's travel that reads as released. */
    val triggerDeadzone: Float = 0f,
) {
    fun apply(s: PadState): PadState {
        if (stickDeadzone <= 0f && triggerDeadzone <= 0f) return s
        val (lx, ly) = stick(s.lx, s.ly)
        val (rx, ry) = stick(s.rx, s.ry)
        return s.copy(lx = lx, ly = ly, rx = rx, ry = ry, leftTrigger = trigger(s.leftTrigger), rightTrigger = trigger(s.rightTrigger))
    }

    /** Radial, so a diagonal behaves like a straight push and small circles don't snap to the axes. */
    private fun stick(x: Int, y: Int): Pair<Int, Int> {
        if (stickDeadzone <= 0f) return x to y
        val magnitude = hypot(x.toDouble(), y.toDouble()) / STICK_MAX
        if (magnitude <= stickDeadzone) return 0 to 0
        val scaled = ((magnitude - stickDeadzone) / (1 - stickDeadzone)).coerceAtMost(1.0)
        val factor = scaled / magnitude
        return clampStick(x * factor) to clampStick(y * factor)
    }

    private fun trigger(t: Int): Int {
        if (triggerDeadzone <= 0f) return t
        val v = t / 255f
        if (v <= triggerDeadzone) return 0
        return ((v - triggerDeadzone) / (1 - triggerDeadzone) * 255f).roundToInt().coerceIn(0, 255)
    }

    private fun clampStick(v: Double) = v.roundToInt().coerceIn(-STICK_MAX.toInt(), STICK_MAX.toInt())

    companion object {
        private const val STICK_MAX = 32767.0
        /** The sliders go up to this much of the travel. */
        const val MAX_DEADZONE_PERCENT = 30
    }
}
