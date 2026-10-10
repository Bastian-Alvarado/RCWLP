package com.rcwlp.controller

/**
 * Gives each attached controller a slot (0 to [Proto.MAX_PADS] - 1), the lowest one free, and
 * keeps it until that controller goes away. The PC then sees each physical controller as the
 * same virtual one for as long as it stays attached, even when others come and go.
 */
class SlotAssigner<K> {
    private val slots = arrayOfNulls<Any>(Proto.MAX_PADS)

    /** The controller's slot, assigning one if it has none. Null when all slots are taken. */
    @Synchronized
    fun assign(key: K): Int? {
        slotOf(key)?.let { return it }
        val free = slots.indexOfFirst { it == null }
        if (free < 0) return null
        slots[free] = key
        return free
    }

    @Synchronized
    fun slotOf(key: K): Int? = slots.indexOf(key).takeIf { it >= 0 }

    /** Frees the controller's slot and returns it, or null if it had none. */
    @Synchronized
    fun release(key: K): Int? = slotOf(key)?.also { slots[it] = null }

    /** (slot, controller) for every taken slot, in slot order. */
    @Suppress("UNCHECKED_CAST")
    @Synchronized
    fun assigned(): List<Pair<Int, K>> = slots.withIndex().mapNotNull { (i, k) -> (k as K?)?.let { i to it } }
}

/**
 * Turns fingers on the phone screen into DualShock 4 touchpad touches, and a quick tap into a
 * touchpad click. Coordinates are scaled from the screen to the touchpad's 1920 × 943.
 */
class Touchpad {
    /** Pointer id → finger index (0 or 1), so a finger keeps its index while it moves. */
    private val fingers = HashMap<Int, Int>()
    private var tapStart = 0L
    private var tapX = 0f
    private var tapY = 0f
    private var tapCandidate = false

    var touch = Touch()
        private set

    /**
     * Feeds the pointers currently down, as (pointer id, x, y) in screen pixels, at time [nowMs].
     * Returns true if this ended a tap, which the caller turns into a short click.
     */
    fun update(pointers: List<Triple<Int, Float, Float>>, width: Int, height: Int, nowMs: Long): Boolean {
        val ids = pointers.map { it.first }.toSet()
        fingers.keys.retainAll(ids)
        for ((id, _, _) in pointers) {
            if (id !in fingers) {
                val free = (0..1).firstOrNull { it !in fingers.values } ?: continue
                fingers[id] = free
            }
        }
        val positions = arrayOfNulls<Pair<Int, Int>>(2)
        for ((id, x, y) in pointers) {
            val finger = fingers[id] ?: continue
            positions[finger] = scale(x, width, Touch.WIDTH) to scale(y, height, Touch.HEIGHT)
        }
        touch = Touch(positions[0], positions[1])

        // A tap: one finger, down briefly, barely moving.
        var tapped = false
        when {
            pointers.size == 1 && !tapCandidate && fingers.size == 1 && tapStart == 0L -> {
                tapCandidate = true
                tapStart = nowMs
                tapX = pointers[0].second
                tapY = pointers[0].third
            }
            pointers.size > 1 -> tapCandidate = false
            pointers.size == 1 && tapCandidate -> {
                val moved = Math.hypot((pointers[0].second - tapX).toDouble(), (pointers[0].third - tapY).toDouble())
                if (moved > TAP_SLOP_PX) tapCandidate = false
            }
            pointers.isEmpty() -> {
                tapped = tapCandidate && nowMs - tapStart <= TAP_MAX_MS
                tapCandidate = false
                tapStart = 0L
            }
        }
        return tapped
    }

    private fun scale(v: Float, size: Int, range: Int): Int =
        if (size <= 0) 0 else (v / size * range).toInt().coerceIn(0, range - 1)

    companion object {
        const val TAP_MAX_MS = 200L
        const val TAP_SLOP_PX = 24.0
        /** How long a tap holds the touchpad click down. */
        const val CLICK_MS = 80L
    }
}
