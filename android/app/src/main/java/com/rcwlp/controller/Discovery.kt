package com.rcwlp.controller

import android.os.SystemClock
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.net.SocketTimeoutException
import java.util.concurrent.ConcurrentHashMap

/** Finds RCWLP hosts on the local network. Blocking; call off the main thread. */
object Discovery {
    /**
     * Every host that answered since the app started, by id. Replies aren't authenticated, so
     * this is only a hint: [ControllerSession] tries the address and saves it once the host proves itself.
     */
    val lastSeen = ConcurrentHashMap<String, FoundHost>()

    /** Broadcasts on every interface and also asks known hosts directly, in case broadcast is blocked. */
    fun scan(known: List<PairedHost>, timeoutMs: Long = 1500): List<FoundHost> =
        query(broadcastTargets() + known.mapNotNull { resolve(it.address, it.port) }, timeoutMs)

    /** Asks one address directly, for networks where broadcast doesn't work. */
    fun probe(address: String, port: Int = Proto.DEFAULT_PORT): FoundHost? =
        resolve(address, port)?.let { query(listOf(it), 1500).firstOrNull() }

    /**
     * DISCOVER in this app's protocol version and in the oldest one still worth recognizing.
     * Hosts answer the version they speak, so an outdated host still shows up, marked as such.
     */
    val requests: List<ByteArray> = (Proto.VERSION downTo Proto.OLDEST_DISCOVERABLE).map {
        PacketWriter(Proto.DISCOVER, version = it).finish()
    }

    /** Reads a DISCOVER_REPLY of any protocol version, or returns null. */
    fun parseReply(buf: ByteArray, len: Int, address: String, port: Int): FoundHost? {
        val r = PacketReader(buf, len)
        val (version, kind) = r.headerAnyVersion() ?: return null
        if (kind != Proto.DISCOVER_REPLY) return null
        val id = r.bytes(Proto.ID_LEN)?.toHex() ?: return null
        val name = r.string() ?: return null
        // Hosts from 0.2.0 on add their own version.
        val hostVersion = if (r.remaining > 0) r.string() else null
        return FoundHost(id, name, address, port, version, hostVersion)
    }

    private fun query(targets: List<InetSocketAddress>, timeoutMs: Long): List<FoundHost> {
        val found = LinkedHashMap<String, FoundHost>()
        DatagramSocket().use { socket ->
            socket.broadcast = true
            socket.soTimeout = 100
            val buf = ByteArray(512)
            val incoming = DatagramPacket(buf, buf.size)
            val start = SystemClock.elapsedRealtime()
            var rounds = 0
            while (true) {
                val elapsed = SystemClock.elapsedRealtime() - start
                if (elapsed > timeoutMs) break
                // A few rounds, since UDP broadcasts get dropped on busy Wi-Fi.
                if (rounds < 3 && elapsed >= rounds * 400L) {
                    for (target in targets) {
                        for (request in requests) runCatching { socket.send(DatagramPacket(request, request.size, target)) }
                    }
                    rounds++
                }
                try {
                    socket.receive(incoming)
                } catch (_: SocketTimeoutException) {
                    continue
                }
                val address = incoming.address.hostAddress ?: continue
                val host = parseReply(buf, incoming.length, address, incoming.port) ?: continue
                // A current host answers both requests; keep whichever comes first.
                found.getOrPut(host.idHex) { host.also { if (it.compatible) lastSeen[it.idHex] = it } }
            }
        }
        return found.values.toList()
    }

    private fun broadcastTargets(): List<InetSocketAddress> {
        val addresses = mutableSetOf<InetAddress>(InetAddress.getByAddress(byteArrayOf(-1, -1, -1, -1)))
        runCatching {
            NetworkInterface.getNetworkInterfaces()?.toList().orEmpty()
                .filter { it.isUp && !it.isLoopback }
                .flatMap { it.interfaceAddresses }
                .mapNotNullTo(addresses) { it.broadcast }
        }
        return addresses.map { InetSocketAddress(it, Proto.DEFAULT_PORT) }
    }

    private fun resolve(address: String, port: Int): InetSocketAddress? =
        runCatching { InetSocketAddress(InetAddress.getByName(address), port) }.getOrNull()
}
