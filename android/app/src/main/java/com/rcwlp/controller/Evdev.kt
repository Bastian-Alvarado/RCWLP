package com.rcwlp.controller

/**
 * Raw Linux input device access (see src/main/cpp/evdev.c). Opening /dev/input needs shell or
 * root privileges, so this is only usable inside [EvdevService].
 */
object Evdev {
    init {
        System.loadLibrary("rcwlp_evdev")
    }

    const val EV_SYN = 0
    const val EV_KEY = 1
    const val EV_ABS = 3
    const val SYN_REPORT = 0
    const val SYN_DROPPED = 3

    /** BTN_SOUTH..BTN_THUMBR: the Linux gamepad button range. */
    val GAMEPAD_BUTTONS = 0x130..0x13e

    /** Returns an fd, or -errno. */
    @JvmStatic external fun open(path: String): Int
    @JvmStatic external fun close(fd: Int)
    @JvmStatic external fun name(fd: Int): String?
    /** [bustype, vendor, product, version] */
    @JvmStatic external fun id(fd: Int): IntArray?
    @JvmStatic external fun keyBits(fd: Int): ByteArray?
    @JvmStatic external fun absBits(fd: Int): ByteArray?
    /** [value, min, max, fuzz, flat, resolution] */
    @JvmStatic external fun absInfo(fd: Int, axis: Int): IntArray?
    /** Returns 0 or -errno. */
    @JvmStatic external fun grab(fd: Int, on: Boolean): Int
    /** Fills [out] with [type, code, value] triples; returns the count, 0 on timeout, or -errno. */
    @JvmStatic external fun read(fd: Int, out: IntArray, timeoutMs: Int): Int

    fun hasBit(bits: ByteArray?, code: Int): Boolean =
        bits != null && code / 8 < bits.size && (bits[code / 8].toInt() shr (code % 8)) and 1 == 1
}
