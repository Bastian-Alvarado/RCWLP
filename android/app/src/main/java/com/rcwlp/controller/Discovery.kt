package com.rcwlp.controller

import android.os.SystemClock
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.net.SocketTimeoutException

/** Finds RCWLP hosts on the local network. Blocking; call off the main thread. */
object Discovery {
    /** Broadcasts on every interface and also asks known hosts directly, in case broadcast is blocked. */
    fun scan(known: List<PairedHost>, timeoutMs: Long = 1500): List<FoundHost> =
        query(broadcastTargets() + known.mapNotNull { resolve(it.address, it.port) }, timeoutMs)

    /** Asks one address directly, for networks where broadcast doesn't work. */
    fun probe(address: String, port: Int = Proto.DEFAULT_PORT): FoundHost? =
        resolve(address, port)?.let { query(listOf(it), 1500).firstOrNull() }

    private fun query(targets: List<InetSocketAddress>, timeoutMs: Long): List<FoundHost> {
        val found = LinkedHashMap<String, FoundHost>()
        val request = PacketWriter(Proto.DISCOVER).finish()
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
                    targets.forEach { runCatching { socket.send(DatagramPacket(request, request.size, it)) } }
                    rounds++
                }
                try {
                    socket.receive(incoming)
                } catch (_: SocketTimeoutException) {
                    continue
                }
                val r = PacketReader(buf, incoming.length)
                if (r.header() != Proto.DISCOVER_REPLY) continue
                val id = r.bytes(Proto.ID_LEN)?.toHex() ?: continue
                val name = r.string() ?: continue
                val address = incoming.address.hostAddress ?: continue
                found.getOrPut(id) { FoundHost(id, name, address, incoming.port) }
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
