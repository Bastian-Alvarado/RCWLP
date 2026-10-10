package com.rcwlp.controller

import android.os.SystemClock
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.SocketTimeoutException
import java.security.MessageDigest

/**
 * Pairs this phone with a host. Both sides run ECDH and derive a code from the resulting key;
 * the phone shows it and the user types it on the PC, which accepts only if its own code matches.
 * Blocking; run off the main thread.
 */
class Pairing(private val store: HostStore, private val host: FoundHost, private val deviceName: String) {
    sealed interface Result {
        class Paired(val host: PairedHost) : Result
        object Rejected : Result
        object TimedOut : Result
        object Cancelled : Result
        class Failed(val message: String) : Result
    }

    @Volatile
    var cancelled = false

    fun run(onCode: (Int) -> Unit): Result = try {
        exchange(onCode)
    } catch (e: Exception) {
        Result.Failed(e.message ?: e.javaClass.simpleName)
    }

    private fun exchange(onCode: (Int) -> Unit): Result {
        val key = Crypto.EphemeralKey()
        val clientId = store.clientId
        val hostAddress = InetAddress.getByName(host.address)
        // The hello only commits to our key; we reveal it after the host has sent its own.
        val hello = Packets.pairHello(clientId, key.publicBytes, deviceName)
        val reveal = Packets.pairReveal(clientId, key.publicBytes)
        var pairKey: ByteArray? = null

        DatagramSocket().use { socket ->
            socket.soTimeout = 250
            val buf = ByteArray(512)
            val incoming = DatagramPacket(buf, buf.size)
            val deadline = SystemClock.elapsedRealtime() + TIMEOUT_MS
            var nextSend = 0L
            while (!cancelled) {
                val now = SystemClock.elapsedRealtime()
                if (now > deadline) return Result.TimedOut
                // Hello until the host's key arrives, then reveal until the result does. Retrying
                // covers lost packets in both directions.
                if (now >= nextSend) {
                    val packet = if (pairKey == null) hello else reveal
                    socket.send(DatagramPacket(packet, packet.size, hostAddress, host.port))
                    nextSend = now + 1000
                }
                try {
                    socket.receive(incoming)
                } catch (_: SocketTimeoutException) {
                    continue
                }
                if (incoming.address != hostAddress) continue
                val r = PacketReader(buf, incoming.length)
                when (r.header()) {
                    Proto.PAIR_CHALLENGE -> {
                        if (pairKey != null) continue
                        val hostId = r.bytes(Proto.ID_LEN) ?: continue
                        val hostPub = r.bytes(Proto.PUB_LEN) ?: continue
                        if (hostId.toHex() != host.idHex) continue
                        val shared = runCatching { key.sharedSecret(hostPub) }.getOrNull() ?: continue
                        val k = Crypto.pairKey(shared, clientId, hostId, key.publicBytes, hostPub)
                        pairKey = k
                        nextSend = 0 // Reveal right away.
                        onCode(Crypto.pairCode(k))
                    }
                    Proto.PAIR_RESULT -> {
                        val status = r.u8() ?: continue
                        val mac = r.bytes(Proto.MAC_LEN) ?: continue
                        if (status == 0) return Result.Rejected
                        val k = pairKey ?: continue
                        if (!MessageDigest.isEqual(mac, Crypto.pairOkMac(k))) continue
                        val paired = PairedHost(host.idHex, host.name, k, host.address, host.port)
                        store.save(paired)
                        return Result.Paired(paired)
                    }
                }
            }
        }
        return Result.Cancelled
    }

    private companion object {
        const val TIMEOUT_MS = 90_000L
    }
}

/** Asks a host to forget this phone, as the phone forgets it. */
object Unpairing {
    /** Sends UNPAIR a few times; returns true once the host confirms. Blocking, about a second at most. */
    fun run(clientId: ByteArray, host: PairedHost): Boolean {
        val request = Packets.unpair(clientId, host.key)
        // The host signs its answer with the same key, so only the real host can confirm.
        val confirmation = Packets.unpaired(clientId, host.key)
        val seen = Discovery.lastSeen[host.idHex]
        val targets = listOfNotNull(resolve(host.address, host.port), seen?.let { resolve(it.address, it.port) }).distinct()
        if (targets.isEmpty()) return false
        DatagramSocket().use { socket ->
            socket.soTimeout = 250
            val buf = ByteArray(128)
            val incoming = DatagramPacket(buf, buf.size)
            repeat(ATTEMPTS) {
                targets.forEach { socket.send(DatagramPacket(request, request.size, it)) }
                try {
                    socket.receive(incoming)
                    if (buf.copyOf(incoming.length).contentEquals(confirmation)) return true
                } catch (_: SocketTimeoutException) {
                }
            }
        }
        return false
    }

    private fun resolve(address: String, port: Int): InetSocketAddress? =
        runCatching { InetSocketAddress(InetAddress.getByName(address), port) }.getOrNull()

    private const val ATTEMPTS = 4
}
