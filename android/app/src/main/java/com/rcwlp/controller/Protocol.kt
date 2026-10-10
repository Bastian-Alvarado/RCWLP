package com.rcwlp.controller

import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import javax.crypto.Mac

/** Wire format shared with the PC host. See PROTOCOL.md at the repo root. All integers are big-endian. */
object Proto {
    val MAGIC = "RCWL".toByteArray(Charsets.US_ASCII)
    const val VERSION = 4
    /** Discovery also asks in this version, so a host that old answers and the app can say it's outdated. */
    const val OLDEST_DISCOVERABLE = 2
    const val DEFAULT_PORT = 47391
    const val MAC_LEN = 16
    const val PUB_LEN = 65
    const val ID_LEN = 16
    const val NONCE_LEN = 16
    const val MAX_NAME = 64
    /** Controllers per phone. */
    const val MAX_PADS = 4

    const val DISCOVER = 0x01
    const val DISCOVER_REPLY = 0x02
    const val PAIR_HELLO = 0x10
    const val PAIR_CHALLENGE = 0x11
    const val PAIR_RESULT = 0x12
    const val PAIR_REVEAL = 0x13
    const val CONNECT = 0x20
    const val WELCOME = 0x21
    const val CONNECT_REJECT = 0x22
    const val INPUT = 0x30
    const val STATUS = 0x31
    const val DISCONNECT = 0x40
    const val UNPAIR = 0x41
    const val UNPAIRED = 0x42

    /** Virtual controller types requested in CONNECT. */
    const val PAD_XBOX360 = 0
    const val PAD_DUALSHOCK4 = 1

    const val RTT_UNKNOWN = 0xFFFF
    const val BATTERY_UNKNOWN = 0x7F
    const val BATTERY_CHARGING = 0x80

    /** One controller in INPUT: slot, buttons, triggers, sticks, motion, touch. */
    const val PAD_LEN = 1 + 2 + 2 + 8 + 12 + 1 + 8
    /** The longest INPUT, with every slot used, MAC included. */
    const val INPUT_MAX_LEN = 6 + ID_LEN + 4 + 2 + 1 + 1 + MAX_PADS * PAD_LEN + MAC_LEN

    /** Splits an authenticated packet into its signed part and trailing MAC. */
    fun splitMac(buf: ByteArray, len: Int): Pair<ByteArray, ByteArray>? =
        if (len <= MAC_LEN) null
        else buf.copyOfRange(0, len - MAC_LEN) to buf.copyOfRange(len - MAC_LEN, len)
}

/** Fingers on the DualShock 4 touchpad, in its coordinates (x 0–1919, y 0–942); null while lifted. */
data class Touch(val finger0: Pair<Int, Int>? = null, val finger1: Pair<Int, Int>? = null) {
    companion object {
        const val WIDTH = 1920
        const val HEIGHT = 943
    }
}

/** One controller's slot in INPUT. Motion and touch belong to the phone, so only slot 0 has them. */
data class PadReport(val slot: Int, val pad: PadState, val motion: Motion = Motion(), val touch: Touch = Touch())

/** What the host sends back for one controller. [lightbar] is 0xRRGGBB, 0 when a game set none. */
data class Feedback(val slot: Int, val large: Int, val small: Int, val player: Int, val lightbar: Int)

/** A STATUS packet's body, after its MAC was checked. */
class Status(val seq: Long, val ackSeq: Long, val ackAgeMs: Int, val pads: List<Feedback>) {
    companion object {
        /** Reads the body after the header. */
        fun read(r: PacketReader): Status? {
            val seq = r.u32() ?: return null
            val ackSeq = r.u32() ?: return null
            val ackAge = r.u16() ?: return null
            val count = r.u8() ?: return null
            val pads = (0 until count).map {
                val slot = r.u8() ?: return null
                val large = r.u8() ?: return null
                val small = r.u8() ?: return null
                val player = r.u8() ?: return null
                val red = r.u8() ?: return null
                val green = r.u8() ?: return null
                val blue = r.u8() ?: return null
                Feedback(slot, large, small, player, red shl 16 or (green shl 8) or blue)
            }
            return Status(seq, ackSeq, ackAge, pads)
        }
    }
}

/** The packets the phone sends, in one place so the protocol test vectors check what the app really sends. */
object Packets {
    /** Pairing step 1: commits to [publicKey] without revealing it (see [Crypto.commitment]). */
    fun pairHello(clientId: ByteArray, publicKey: ByteArray, name: String): ByteArray =
        PacketWriter(Proto.PAIR_HELLO).bytes(clientId).bytes(Crypto.commitment(publicKey)).string(name).finish()

    /** Pairing step 2, sent once the host's key has arrived. */
    fun pairReveal(clientId: ByteArray, publicKey: ByteArray): ByteArray =
        PacketWriter(Proto.PAIR_REVEAL).bytes(clientId).bytes(publicKey).finish()

    fun connect(clientId: ByteArray, cnonce: ByteArray, padType: Int, pairKey: ByteArray): ByteArray =
        PacketWriter(Proto.CONNECT).bytes(clientId).bytes(cnonce).u8(padType).sign(pairKey)

    /**
     * A signed INPUT packet. Written into [buf] (at least [Proto.INPUT_MAX_LEN] bytes) to keep
     * the per-packet work small; returns the packet's length.
     */
    fun input(
        buf: ByteBuffer, mac: Mac, clientId: ByteArray, seq: Int, rttMs: Int, battery: Int, pads: List<PadReport>,
    ): Int {
        buf.clear()
        buf.put(Proto.MAGIC).put(Proto.VERSION.toByte()).put(Proto.INPUT.toByte())
            .put(clientId).putInt(seq)
            .putShort(rttMs.toShort()).put(battery.toByte()).put(pads.size.toByte())
        for ((slot, p, m, t) in pads) {
            buf.put(slot.toByte())
                .putShort(p.buttons.toShort()).put(p.leftTrigger.toByte()).put(p.rightTrigger.toByte())
                .putShort(p.lx.toShort()).putShort(p.ly.toShort()).putShort(p.rx.toShort()).putShort(p.ry.toShort())
                .putShort(m.gx.toShort()).putShort(m.gy.toShort()).putShort(m.gz.toShort())
                .putShort(m.ax.toShort()).putShort(m.ay.toShort()).putShort(m.az.toShort())
            val down = (if (t.finger0 != null) 1 else 0) or (if (t.finger1 != null) 2 else 0)
            buf.put(down.toByte())
            for (finger in listOf(t.finger0, t.finger1)) {
                buf.putShort((finger?.first ?: 0).toShort()).putShort((finger?.second ?: 0).toShort())
            }
        }
        val bodyLen = buf.position()
        mac.update(buf.array(), 0, bodyLen)
        buf.put(mac.doFinal(), 0, Proto.MAC_LEN)
        return buf.position()
    }

    fun disconnect(clientId: ByteArray, seq: Int, sessionKey: ByteArray): ByteArray =
        PacketWriter(Proto.DISCONNECT).bytes(clientId).u32(seq).sign(sessionKey)

    /** Asks the host to forget this phone; signed with the pair key, which is about to be deleted. */
    fun unpair(clientId: ByteArray, pairKey: ByteArray): ByteArray =
        PacketWriter(Proto.UNPAIR).bytes(clientId).sign(pairKey)

    /** The host's answer to [unpair], signed with the same key so the phone knows it was the real host. */
    fun unpaired(clientId: ByteArray, pairKey: ByteArray): ByteArray =
        PacketWriter(Proto.UNPAIRED).bytes(clientId).sign(pairKey)
}

/** [version] is only ever different for DISCOVER, which works across versions. */
class PacketWriter(kind: Int, version: Int = Proto.VERSION) {
    private val out = ByteArrayOutputStream(64)

    init {
        out.write(Proto.MAGIC)
        out.write(version)
        out.write(kind)
    }

    fun u8(v: Int) = apply { out.write(v) }

    fun u16(v: Int) = apply { out.write(ByteBuffer.allocate(2).putShort(v.toShort()).array()) }

    fun u32(v: Int) = apply { out.write(ByteBuffer.allocate(4).putInt(v).array()) }

    fun bytes(v: ByteArray) = apply { out.write(v) }

    fun string(s: String) = apply {
        var end = s.length
        while (end > 0 && s.substring(0, end).toByteArray(Charsets.UTF_8).size > Proto.MAX_NAME) end--
        if (end in 1 until s.length && Character.isHighSurrogate(s[end - 1])) end--
        val b = s.substring(0, end).toByteArray(Charsets.UTF_8)
        out.write(b.size)
        out.write(b)
    }

    fun finish(): ByteArray = out.toByteArray()

    /** Appends a MAC over everything written so far. */
    fun sign(key: ByteArray): ByteArray = finish().let { it + Crypto.mac16(key, it) }
}

class PacketReader(private val buf: ByteArray, private val end: Int = buf.size) {
    private var pos = 0

    /** Checks magic and version, returns the packet kind. */
    fun header(): Int? {
        val (version, kind) = headerAnyVersion() ?: return null
        return if (version == Proto.VERSION) kind else null
    }

    /** Checks only the magic and returns (version, kind), so discovery can spot a host of another version. */
    fun headerAnyVersion(): Pair<Int, Int>? {
        val magic = bytes(4) ?: return null
        if (!magic.contentEquals(Proto.MAGIC)) return null
        val version = u8() ?: return null
        val kind = u8() ?: return null
        return version to kind
    }

    val remaining get() = end - pos

    fun bytes(n: Int): ByteArray? {
        if (pos + n > end) return null
        return buf.copyOfRange(pos, pos + n).also { pos += n }
    }

    fun u8(): Int? = if (pos < end) buf[pos++].toInt() and 0xff else null

    fun u16(): Int? = bytes(2)?.let { ByteBuffer.wrap(it).short.toInt() and 0xffff }

    fun u32(): Long? = bytes(4)?.let { ByteBuffer.wrap(it).int.toLong() and 0xffffffffL }

    fun string(): String? {
        val n = u8() ?: return null
        return bytes(n)?.toString(Charsets.UTF_8)
    }
}

fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }

fun String.hexToBytes(): ByteArray = ByteArray(length / 2) { substring(it * 2, it * 2 + 2).toInt(16).toByte() }
