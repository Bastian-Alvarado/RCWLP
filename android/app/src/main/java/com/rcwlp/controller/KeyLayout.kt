package com.rcwlp.controller

import android.view.KeyEvent
import android.view.MotionEvent
import java.io.File

/**
 * Android's key layout (.kl) for one input device: how its raw scan codes and axes become
 * keycodes and MotionEvent axes. Reading raw evdev bypasses Android's input stack, so we apply
 * the same layout file Android would, and buttons map exactly as they do in foreground mode.
 */
class KeyLayout(val keys: Map<Int, Int>, val axes: Map<Int, Axis>) {
    /** How one raw axis maps: to [axis] (optionally inverted), or split in two at [splitValue]. */
    data class Axis(
        val axis: Int,
        val invert: Boolean = false,
        val splitValue: Int? = null,
        val highAxis: Int = -1,
        val flat: Int? = null,
    )

    companion object {
        private val DIRS = listOf("/product/usr/", "/system_ext/usr/", "/odm/usr/", "/vendor/usr/", "/system/usr/", "/data/system/devices/")

        /** Finds the layout the way Android's InputDevice.cpp does, falling back to Generic.kl. */
        fun load(vendor: Int, product: Int, version: Int, name: String): KeyLayout {
            val candidates = buildList {
                if (vendor != 0 && product != 0) {
                    if (version != 0) add("Vendor_%04x_Product_%04x_Version_%04x".format(vendor, product, version))
                    add("Vendor_%04x_Product_%04x".format(vendor, product))
                }
                add(name.map { if (it.isLetterOrDigit() || it in "-_.") it else '_' }.joinToString(""))
                add("Generic")
            }
            for (candidate in candidates) {
                for (dir in DIRS) {
                    val file = File("${dir}keylayout/$candidate.kl")
                    val text = runCatching { file.readText() }.getOrNull() ?: continue
                    return parse(text)
                }
            }
            return KeyLayout(emptyMap(), emptyMap())
        }

        fun parse(text: String): KeyLayout {
            val keys = HashMap<Int, Int>()
            val axes = HashMap<Int, Axis>()
            for (raw in text.lineSequence()) {
                val t = raw.substringBefore('#').trim().split(Regex("\\s+")).filter { it.isNotEmpty() }
                if (t.size < 3) continue
                when (t[0]) {
                    "key" -> {
                        if (t[1] == "usage") continue // HID usage mappings; evdev gives scan codes
                        val code = t[1].toCodeOrNull() ?: continue
                        val keyCode = KeyEvent.keyCodeFromString("KEYCODE_${t[2]}")
                        if (keyCode != KeyEvent.KEYCODE_UNKNOWN) keys[code] = keyCode
                    }
                    "axis" -> parseAxis(t)?.let { (code, axis) -> axes[code] = axis }
                }
            }
            return KeyLayout(keys, axes)
        }

        private fun parseAxis(t: List<String>): Pair<Int, Axis>? {
            val code = t[1].toCodeOrNull() ?: return null
            var i = 2
            val axis = when (t[i]) {
                "invert" -> Axis(axisOf(t.getOrNull(++i)) ?: return null, invert = true).also { i++ }
                "split" -> {
                    val split = t.getOrNull(3)?.toCodeOrNull() ?: return null
                    val low = axisOf(t.getOrNull(4)) ?: return null
                    val high = axisOf(t.getOrNull(5)) ?: return null
                    i = 6
                    Axis(low, splitValue = split, highAxis = high)
                }
                else -> Axis(axisOf(t[i]) ?: return null).also { i++ }
            }
            val flat = if (t.getOrNull(i) == "flat") t.getOrNull(i + 1)?.toIntOrNull() else null
            return code to axis.copy(flat = flat)
        }

        private fun axisOf(name: String?): Int? =
            name?.let { MotionEvent.axisFromString("AXIS_$it") }?.takeIf { it >= 0 }

        private fun String.toCodeOrNull(): Int? = runCatching { Integer.decode(this) }.getOrNull()
    }
}
