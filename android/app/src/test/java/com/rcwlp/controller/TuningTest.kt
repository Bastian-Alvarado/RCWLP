package com.rcwlp.controller

import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.hypot

class TuningTest {
    private val tuning = Tuning(stickDeadzone = 0.2f, triggerDeadzone = 0.1f)

    @Test
    fun noDeadZonesLeavesTheStateAlone() {
        val s = PadState(lx = 1000, leftTrigger = 3)
        assertSame(s, Tuning().apply(s))
    }

    @Test
    fun driftInsideTheDeadZoneReadsAsCentred() {
        val drift = PadState(lx = 4000, ly = -3000, rx = -5000)
        val tuned = tuning.apply(drift)
        assertEquals(0 to 0, tuned.lx to tuned.ly)
        assertEquals(0, tuned.rx)
    }

    @Test
    fun fullDeflectionStillReachesTheMaximum() {
        val tuned = tuning.apply(PadState(lx = 32767, ry = -32767))
        assertEquals(32767, tuned.lx)
        assertEquals(-32767, tuned.ry)
    }

    @Test
    fun theRestOfTheRangeIsStretchedAndKeepsDirection() {
        // Halfway between the dead zone's edge (20%) and full (100%) is 60%.
        val tuned = tuning.apply(PadState(lx = (32767 * 0.6 * 0.6).toInt(), ly = (32767 * 0.6 * 0.8).toInt()))
        val magnitude = hypot(tuned.lx.toDouble(), tuned.ly.toDouble()) / 32767
        assertEquals(0.5, magnitude, 0.01)
        assertEquals(0.6 / 0.8, tuned.lx.toDouble() / tuned.ly, 0.01)
    }

    @Test
    fun triggersIgnoreTheFirstStretchAndStillReachFull() {
        assertEquals(0, tuning.apply(PadState(leftTrigger = 25)).leftTrigger)
        assertEquals(255, tuning.apply(PadState(rightTrigger = 255)).rightTrigger)
        val half = tuning.apply(PadState(leftTrigger = 140)).leftTrigger
        assertTrue("$half", half in 125..135)
    }
}
