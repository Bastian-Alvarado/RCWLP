package com.rcwlp.controller

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.nio.ByteBuffer

/**
 * Checks the app against protocol-vectors.txt at the repo root. The host's cargo tests check the
 * same file, so if both pass, the two sides agree on every key and packet byte.
 */
class ProtocolVectorsTest {
    private val vectors: Map<String, String> =
        File(System.getProperty("rcwlp.vectors") ?: error("Run through Gradle, which sets rcwlp.vectors"))
            .readLines()
            .map { it.substringBefore('#') }
            .filter { '=' in it }
            .associate { it.substringBefore('=').trim() to it.substringAfter('=').trim() }

    private fun str(key: String) = vectors[key] ?: error("protocol-vectors.txt has no $key")
    private fun hex(key: String) = str(key).hexToBytes()
    private fun int(key: String) = str(key).toInt()
    private fun ints(key: String) = str(key).split(Regex("\\s+")).map(String::toInt)

    /** Compares as hex, so a mismatch shows both values readably. */
    private fun check(key: String, actual: ByteArray) = assertEquals(key, str(key), actual.toHex())

    private val clientKey get() = Crypto.EphemeralKey.fromRaw(hex("client_private"), hex("client_public"))

    @Test
    fun derivesTheSameKeysAsTheHost() {
        val key = clientKey
        check("client_public", key.publicBytes)
        val pairKey = Crypto.pairKey(
            key.sharedSecret(hex("host_public")), hex("client_id"), hex("host_id"), key.publicBytes, hex("host_public"),
        )
        check("pair_key", pairKey)
        check("commitment", Crypto.commitment(key.publicBytes))
        assertEquals(int("pair_code"), Crypto.pairCode(pairKey))
        check("pair_ok_mac", Crypto.pairOkMac(pairKey))
        check("session_key", Crypto.sessionKey(pairKey, hex("cnonce"), hex("hnonce")))
    }

    @Test
    fun sendsTheSamePacketBytes() {
        val clientId = hex("client_id")
        val publicKey = clientKey.publicBytes
        check("pair_hello", Packets.pairHello(clientId, publicKey, str("name")))
        check("pair_reveal", Packets.pairReveal(clientId, publicKey))
        check("connect", Packets.connect(clientId, hex("cnonce"), int("pad_type"), hex("pair_key")))

        val pad = PadState(int("buttons"), int("left_trigger"), int("right_trigger"), int("lx"), int("ly"), int("rx"), int("ry"))
        val (gx, gy, gz) = ints("gyro")
        val (ax, ay, az) = ints("accel")
        val buf = ByteBuffer.allocate(Proto.INPUT_BODY_LEN + Proto.MAC_LEN)
        val mac = Crypto.newMac(hex("session_key"))
        check("input", Packets.input(buf, mac, clientId, int("seq"), pad, Motion(gx, gy, gz, ax, ay, az)))
    }

    /** Reads the host's packets the way Pairing and ControllerSession do. */
    @Test
    fun readsTheHostsPackets() {
        val challenge = PacketReader(hex("pair_challenge"))
        assertEquals(Proto.PAIR_CHALLENGE, challenge.header())
        assertArrayEquals(hex("host_id"), challenge.bytes(Proto.ID_LEN))
        assertArrayEquals(hex("host_public"), challenge.bytes(Proto.PUB_LEN))

        val result = PacketReader(hex("pair_result"))
        assertEquals(Proto.PAIR_RESULT, result.header())
        assertEquals(1, result.u8())
        assertArrayEquals(Crypto.pairOkMac(hex("pair_key")), result.bytes(Proto.MAC_LEN))

        val sessionKey = hex("session_key")
        val welcome = hex("welcome")
        val (welcomeBody, welcomeMac) = Proto.splitMac(welcome, welcome.size)!!
        assertTrue("welcome mac", Crypto.verify16(sessionKey, welcomeBody, welcomeMac))
        val w = PacketReader(welcomeBody)
        assertEquals(Proto.WELCOME, w.header())
        assertArrayEquals(hex("hnonce"), w.bytes(Proto.NONCE_LEN))

        val status = hex("status")
        val (statusBody, statusMac) = Proto.splitMac(status, status.size)!!
        assertTrue("status mac", Crypto.verify16(sessionKey, statusBody, statusMac))
        val s = PacketReader(statusBody)
        assertEquals(Proto.STATUS, s.header())
        assertEquals(int("status_seq").toLong(), s.u32())
        assertEquals(int("large_motor"), s.u8())
        assertEquals(int("small_motor"), s.u8())
        assertEquals(int("player"), s.u8())
    }
}
