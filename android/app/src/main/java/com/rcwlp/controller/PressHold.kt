package com.rcwlp.controller

/**
 * Keeps every button press visible for at least [minPressMs]. A quick tap then still reaches the
 * PC when one packet is lost, and a game that only looks at the controller once a frame can't
 * miss it between two looks. Releases that come later than that are passed on as they are.
 */
class PressHold(private val minPressMs: Long = MIN_PRESS_MS) {
    private val pressedAt = LongArray(BITS) { NEVER }
    private var actual = 0

    /** The controller's real buttons at [nowMs]. */
    fun update(buttons: Int, nowMs: Long) {
        val newlyPressed = buttons and actual.inv()
        for (bit in 0 until BITS) {
            if (newlyPressed shr bit and 1 != 0) pressedAt[bit] = nowMs
        }
        actual = buttons
    }

    /** The buttons to send at [nowMs]: the real ones, plus released ones still within their minimum. */
    fun buttons(nowMs: Long): Int {
        var b = actual
        for (bit in 0 until BITS) {
            if (held(bit, nowMs)) b = b or (1 shl bit)
        }
        return b
    }

    /** When the next held press ends, or null if none is held. */
    fun nextRelease(nowMs: Long): Long? =
        (0 until BITS).filter { held(it, nowMs) }.minOfOrNull { pressedAt[it] + minPressMs }

    private fun held(bit: Int, nowMs: Long) =
        actual shr bit and 1 == 0 && pressedAt[bit] != NEVER && nowMs - pressedAt[bit] < minPressMs

    companion object {
        /** About one frame at 60 fps, plus a little. */
        const val MIN_PRESS_MS = 20L
        private const val BITS = 16
        private const val NEVER = Long.MIN_VALUE / 2
    }
}
