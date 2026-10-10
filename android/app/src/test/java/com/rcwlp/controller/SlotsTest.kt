package com.rcwlp.controller

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SlotsTest {
    @Test
    fun controllersKeepTheirSlotWhileOthersComeAndGo() {
        val slots = SlotAssigner<String>()
        assertEquals(0, slots.assign("clip-on"))
        assertEquals(1, slots.assign("bluetooth pad"))
        assertEquals(0, slots.assign("clip-on"), "asking again keeps the slot")
        assertEquals(0, slots.release("clip-on"))
        assertEquals(1, slots.slotOf("bluetooth pad"), "the other controller doesn't move")
        assertEquals(0, slots.assign("new pad"), "the lowest free slot")
        assertEquals(listOf(0 to "new pad", 1 to "bluetooth pad"), slots.assigned())
    }

    @Test
    fun onlyFourFit() {
        val slots = SlotAssigner<Int>()
        repeat(Proto.MAX_PADS) { assertEquals(it, slots.assign(it)) }
        assertNull(slots.assign(99))
        assertNull(slots.release(99))
    }

    private val width = 2000
    private val height = 1000

    @Test
    fun scalesFingersToTheTouchpad() {
        val pad = Touchpad()
        pad.update(listOf(Triple(7, 1000f, 500f)), width, height, 0)
        assertEquals(Touch(960 to 471), pad.touch)
        pad.update(listOf(Triple(7, 1000f, 500f), Triple(9, 1999f, 999f)), width, height, 10)
        assertEquals(Touch(960 to 471, 1919 to 942), pad.touch)
        // The first finger lifts; the second keeps its index.
        pad.update(listOf(Triple(9, 0f, 0f)), width, height, 20)
        assertEquals(Touch(null, 0 to 0), pad.touch)
        pad.update(emptyList(), width, height, 30)
        assertEquals(Touch(), pad.touch)
    }

    @Test
    fun aQuickTapClicks() {
        val pad = Touchpad()
        assertFalse(pad.update(listOf(Triple(1, 100f, 100f)), width, height, 1000))
        assertTrue(pad.update(emptyList(), width, height, 1000 + Touchpad.TAP_MAX_MS))
    }

    @Test
    fun slidingOrHoldingDoesntClick() {
        val pad = Touchpad()
        pad.update(listOf(Triple(1, 100f, 100f)), width, height, 0)
        pad.update(listOf(Triple(1, 300f, 100f)), width, height, 50)
        assertFalse("slid", pad.update(emptyList(), width, height, 100))

        pad.update(listOf(Triple(2, 100f, 100f)), width, height, 1000)
        assertFalse("held", pad.update(emptyList(), width, height, 1000 + Touchpad.TAP_MAX_MS + 1))

        pad.update(listOf(Triple(3, 100f, 100f)), width, height, 2000)
        pad.update(listOf(Triple(3, 100f, 100f), Triple(4, 500f, 500f)), width, height, 2010)
        pad.update(listOf(Triple(3, 100f, 100f)), width, height, 2020)
        assertFalse("two fingers", pad.update(emptyList(), width, height, 2030))
    }

    @Test
    fun controllerKeysTieAControllersInterfacesTogether() {
        // A USB controller's gamepad and consumer-control interfaces share the port part.
        assertEquals(
            Probed.controllerKey("", "usb-xhci-hcd.0.auto-1/input0", "/dev/input/event5"),
            Probed.controllerKey(null, "usb-xhci-hcd.0.auto-1/input2", "/dev/input/event6"),
        )
        // Bluetooth devices all share the phone's adapter as phys; their address tells them apart.
        assertEquals("uniq:aa:bb", Probed.controllerKey("aa:bb", "11:22:33:44:55:66", "/dev/input/event7"))
        assertEquals("path:/dev/input/event8", Probed.controllerKey("", "", "/dev/input/event8"))
    }

    private fun assertEquals(expected: Any?, actual: Any?, message: String) = org.junit.Assert.assertEquals(message, expected, actual)
}
