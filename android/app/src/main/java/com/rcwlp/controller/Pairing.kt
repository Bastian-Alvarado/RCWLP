package com.rcwlp.controller

import android.os.SystemClock
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketTimeoutException
import java.security.MessageDigest

/**
 * Pairs this phone with a host. Both sides run ECDH and show a code derived from the
 * resulting key; the user accepts on the PC once the codes match. Blocking; run off the main thread.
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
        val hello = PacketWriter(Proto.PAIR_HELLO)
            .bytes(clientId)
            .bytes(key.publicBytes)
            .string(deviceName)
            .finish()
        var pairKey: ByteArray? = null

        DatagramSocket().use { socket ->
            socket.soTimeout = 250
            val buf = ByteArray(512)
            val incoming = DatagramPacket(buf, buf.size)
            val deadline = SystemClock.elapsedRealtime() + TIMEOUT_MS
            var nextHello = 0L
            while (!cancelled) {
                val now = SystemClock.elapsedRealtime()
                if (now > deadline) return Result.TimedOut
                // The host answers every hello, so retrying covers lost packets in both directions.
                if (now >= nextHello) {
                    socket.send(DatagramPacket(hello, hello.size, hostAddress, host.port))
                    nextHello = now + 1000
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
