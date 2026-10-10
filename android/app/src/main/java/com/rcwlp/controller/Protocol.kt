package com.rcwlp.controller

import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import javax.crypto.Mac

/** Wire format shared with the PC host. See PROTOCOL.md at the repo root. All integers are big-endian. */
object Proto {
    val MAGIC = "RCWL".toByteArray(Charsets.US_ASCII)
    const val VERSION = 3
    const val DEFAULT_PORT = 47391
    const val MAC_LEN = 16
    const val PUB_LEN = 65
    const val ID_LEN = 16
    const val NONCE_LEN = 16
    const val MAX_NAME = 64

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

    /** Virtual controller types requested in CONNECT. */
    const val PAD_XBOX360 = 0
    const val PAD_DUALSHOCK4 = 1

    /** Header + client id + seq + pad state + motion, without the MAC. */
    const val INPUT_BODY_LEN = 6 + ID_LEN + 4 + 12 + 12

    /** Splits an authenticated packet into its signed part and trailing MAC. */
    fun splitMac(buf: ByteArray, len: Int): Pair<ByteArray, ByteArray>? =
        if (len <= MAC_LEN) null
        else buf.copyOfRange(0, len - MAC_LEN) to buf.copyOfRange(len - MAC_LEN, len)
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

    /** A signed INPUT packet, written into [buf] (INPUT_BODY_LEN + MAC_LEN bytes) to avoid allocating per packet. */
    fun input(buf: ByteBuffer, mac: Mac, clientId: ByteArray, seq: Int, p: PadState, m: Motion): ByteArray {
        buf.clear()
        buf.put(Proto.MAGIC).put(Proto.VERSION.toByte()).put(Proto.INPUT.toByte())
            .put(clientId).putInt(seq)
            .putShort(p.buttons.toShort()).put(p.leftTrigger.toByte()).put(p.rightTrigger.toByte())
            .putShort(p.lx.toShort()).putShort(p.ly.toShort()).putShort(p.rx.toShort()).putShort(p.ry.toShort())
            .putShort(m.gx.toShort()).putShort(m.gy.toShort()).putShort(m.gz.toShort())
            .putShort(m.ax.toShort()).putShort(m.ay.toShort()).putShort(m.az.toShort())
        mac.update(buf.array(), 0, Proto.INPUT_BODY_LEN)
        buf.put(mac.doFinal(), 0, Proto.MAC_LEN)
        return buf.array()
    }

    fun disconnect(clientId: ByteArray, seq: Int, sessionKey: ByteArray): ByteArray =
        PacketWriter(Proto.DISCONNECT).bytes(clientId).u32(seq).sign(sessionKey)
}

class PacketWriter(kind: Int) {
    private val out = ByteArrayOutputStream(64)

    init {
        out.write(Proto.MAGIC)
        out.write(Proto.VERSION)
        out.write(kind)
    }

    fun u8(v: Int) = apply { out.write(v) }

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
        val magic = bytes(4) ?: return null
        if (!magic.contentEquals(Proto.MAGIC) || u8() != Proto.VERSION) return null
        return u8()
    }

    fun bytes(n: Int): ByteArray? {
        if (pos + n > end) return null
        return buf.copyOfRange(pos, pos + n).also { pos += n }
    }

    fun u8(): Int? = if (pos < end) buf[pos++].toInt() and 0xff else null

    fun u32(): Long? = bytes(4)?.let { ByteBuffer.wrap(it).int.toLong() and 0xffffffffL }

    fun string(): String? {
        val n = u8() ?: return null
        return bytes(n)?.toString(Charsets.UTF_8)
    }
}

fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }

fun String.hexToBytes(): ByteArray = ByteArray(length / 2) { substring(it * 2, it * 2 + 2).toInt(16).toByte() }
