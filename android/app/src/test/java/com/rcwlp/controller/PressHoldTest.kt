package com.rcwlp.controller

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PressHoldTest {
    private val a = XButton.A
    private val b = XButton.B

    @Test
    fun aQuickTapIsHeldForTheMinimum() {
        val hold = PressHold(minPressMs = 20)
        hold.update(a, nowMs = 1000)
        hold.update(0, nowMs = 1003) // released after 3 ms
        assertEquals(a, hold.buttons(1003))
        assertEquals(1020L, hold.nextRelease(1003))
        assertEquals(a, hold.buttons(1019))
        assertEquals(0, hold.buttons(1020))
        assertNull(hold.nextRelease(1020))
    }

    @Test
    fun aLongPressIsReleasedRightAway() {
        val hold = PressHold(minPressMs = 20)
        hold.update(a, nowMs = 0)
        hold.update(0, nowMs = 300)
        assertEquals(0, hold.buttons(300))
        assertNull(hold.nextRelease(300))
    }

    @Test
    fun otherButtonsPassThroughWhileOneIsHeld() {
        val hold = PressHold(minPressMs = 20)
        hold.update(a, nowMs = 0)
        hold.update(b, nowMs = 5) // A released, B pressed
        assertEquals(a or b, hold.buttons(5))
        assertEquals(b, hold.buttons(25))
    }

    @Test
    fun aRepeatedTapRestartsTheMinimum() {
        val hold = PressHold(minPressMs = 20)
        hold.update(a, nowMs = 0)
        hold.update(0, nowMs = 2)
        hold.update(a, nowMs = 15)
        hold.update(0, nowMs = 16)
        assertEquals(a, hold.buttons(30))
        assertEquals(0, hold.buttons(35))
    }

    @Test
    fun nothingPressedNothingHeld() {
        val hold = PressHold()
        assertEquals(0, hold.buttons(0))
        assertNull(hold.nextRelease(0))
    }
}
