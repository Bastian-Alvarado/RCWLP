package com.rcwlp.controller

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DiscoveryTest {
    private val id = ByteArray(16) { it.toByte() }

    private fun reply(version: Int, hostVersion: String?): ByteArray {
        val w = PacketWriter(Proto.DISCOVER_REPLY, version).bytes(id).string("PC")
        if (hostVersion != null) w.string(hostVersion)
        return w.finish()
    }

    @Test
    fun asksInEveryVersionItCanRecognize() {
        val versions = Discovery.requests.map { PacketReader(it).headerAnyVersion() }
        assertEquals(listOf(Proto.VERSION to Proto.DISCOVER, Proto.OLDEST_DISCOVERABLE to Proto.DISCOVER), versions)
    }

    @Test
    fun readsACurrentHost() {
        val packet = reply(Proto.VERSION, "0.2.0")
        val host = Discovery.parseReply(packet, packet.size, "192.168.1.5", 47391)!!
        assertEquals(id.toHex(), host.idHex)
        assertEquals("PC", host.name)
        assertEquals("0.2.0", host.hostVersion)
        assertTrue(host.compatible)
        assertNull(host.incompatibility())
    }

    @Test
    fun recognizesAnOlderHostAndSaysToUpdateIt() {
        // 0.1.0 hosts spoke protocol v2 and didn't send their version.
        val packet = reply(2, null)
        val host = Discovery.parseReply(packet, packet.size, "192.168.1.5", 47391)!!
        assertNull(host.hostVersion)
        assertTrue(host.incompatibility()!!.contains("older RCWLP host"))
        assertTrue(host.incompatibility()!!.contains("Update it on the PC"))
    }

    @Test
    fun recognizesANewerHostAndSaysToUpdateTheApp() {
        val packet = reply(Proto.VERSION + 1, "9.0.0")
        val problem = Discovery.parseReply(packet, packet.size, "192.168.1.5", 47391)!!.incompatibility()
        assertNotNull(problem)
        assertTrue(problem!!.contains("newer RCWLP host (9.0.0)"))
        assertTrue(problem.contains("Update this app"))
    }

    @Test
    fun ignoresOtherPackets() {
        val packet = PacketWriter(Proto.STATUS).u32(1).finish()
        assertNull(Discovery.parseReply(packet, packet.size, "x", 1))
        assertNull(Discovery.parseReply(ByteArray(3), 3, "x", 1))
    }
}
