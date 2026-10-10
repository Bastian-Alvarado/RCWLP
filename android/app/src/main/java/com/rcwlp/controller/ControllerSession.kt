package com.rcwlp.controller

import android.os.SystemClock
import java.io.IOException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.SocketTimeoutException
import java.nio.ByteBuffer
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

/**
 * Streams controller state to one paired host and plays its rumble.
 *
 * The sender thread connects, then sends the full pad state on every change plus a heartbeat
 * every [HEARTBEAT_MS]. Each packet carries the whole state, so a lost packet costs nothing.
 * The receiver thread verifies the host's WELCOME and STATUS packets.
 *
 * The host is tried at its saved address and wherever discovery last saw it. Discovery replies
 * aren't authenticated, so only the address that answers with a valid WELCOME is used and saved.
 */
class ControllerSession(
    private val store: HostStore,
    private val host: PairedHost,
    /** [Proto.PAD_XBOX360] or [Proto.PAD_DUALSHOCK4]. */
    private val padType: Int,
    private val listener: Listener,
) {
    enum class State { CONNECTING, CONNECTED, NOT_PAIRED }

    /** Called from network threads. */
    interface Listener {
        fun onState(state: State, player: Int)
        fun onRumble(large: Int, small: Int)
    }

    private val clientId = store.clientId
    @Volatile private var running = false
    @Volatile private var pad = PadState()
    @Volatile private var motion = Motion()
    private val wake = ArrayBlockingQueue<Unit>(1)
    /** Wakes the sender while it waits for WELCOME or CONNECT_REJECT. */
    private val connectSignal = ArrayBlockingQueue<Unit>(1)
    private val socket = DatagramSocket()
    /** Addresses to try, saved one first. Set before the receiver starts. */
    private var candidates: List<InetSocketAddress> = emptyList()

    // Set by the receiver, read by the sender.
    @Volatile private var cnonce = ByteArray(0)
    /** The candidate that proved it's the host. Set before [sessionKey]. */
    @Volatile private var hostAddress: InetSocketAddress? = null
    @Volatile private var sessionKey: ByteArray? = null
    @Volatile private var rejected = false
    @Volatile private var lastStatusAt = 0L
    private val rejectedBy = ConcurrentHashMap.newKeySet<InetSocketAddress>()

    // Receiver thread only.
    private var lastStatusSeq = 0L
    private var reportedPlayer = -1

    // Sender thread only.
    private var seq = 0

    fun start() {
        running = true
        socket.soTimeout = 200
        thread(name = "rcwlp-send") { sendLoop() }
    }

    fun stop() {
        running = false
        wake.offer(Unit)
        connectSignal.offer(Unit)
    }

    fun update(state: PadState) {
        if (state == pad) return
        pad = state
        wake.offer(Unit)
    }

    /** Called from the sensor thread. */
    fun update(state: Motion) {
        motion = state
        wake.offer(Unit)
    }

    private fun sendLoop() {
        try {
            val seen = Discovery.lastSeen[host.idHex]
            candidates = listOfNotNull(resolve(host.address, host.port), seen?.let { resolve(it.address, it.port) })
                .distinct()
            if (candidates.isEmpty()) return
            thread(name = "rcwlp-recv") { receiveLoop() }
            while (running && connect()) streamInput()
            sendDisconnect()
        } catch (_: IOException) {
            // Socket closed or network gone; the activity restarts us when it comes back.
        } finally {
            running = false
            socket.close()
        }
    }

    /** Returns true once the host has welcomed us. */
    private fun connect(): Boolean {
        listener.onState(State.CONNECTING, 0)
        sessionKey = null
        hostAddress = null
        rejectedBy.clear()
        cnonce = Crypto.random(Proto.NONCE_LEN)
        val packet = Packets.connect(clientId, cnonce, padType, host.key)
        while (running) {
            if (sessionKey != null) return true
            if (rejected) {
                listener.onState(State.NOT_PAIRED, 0)
                return false
            }
            candidates.forEach { send(packet, it) }
            connectSignal.poll(CONNECT_RETRY_MS, TimeUnit.MILLISECONDS)
        }
        return false
    }

    private fun streamInput() {
        val key = sessionKey ?: return
        val target = hostAddress ?: return
        val mac = Crypto.newMac(key)
        val buf = ByteBuffer.allocate(Proto.INPUT_BODY_LEN + Proto.MAC_LEN)
        seq = 0
        lastStatusAt = SystemClock.elapsedRealtime()
        while (running) {
            seq++
            send(Packets.input(buf, mac, clientId, seq, pad, motion), target)
            val sentAt = SystemClock.elapsedRealtime()

            if (sentAt - lastStatusAt > LOST_AFTER_MS) {
                listener.onRumble(0, 0)
                return // Host went quiet: start a fresh session.
            }
            wake.poll(HEARTBEAT_MS, TimeUnit.MILLISECONDS)
            // Sensors fire hundreds of times a second; batch them into at most one packet per interval.
            val wait = MIN_INTERVAL_MS - (SystemClock.elapsedRealtime() - sentAt)
            if (wait > 0) Thread.sleep(wait)
        }
    }

    private fun sendDisconnect() {
        val key = sessionKey ?: return
        val target = hostAddress ?: return
        val packet = Packets.disconnect(clientId, seq + 1, key)
        repeat(3) { runCatching { send(packet, target) } }
    }

    private fun send(data: ByteArray, to: InetSocketAddress) {
        socket.send(DatagramPacket(data, data.size, to))
    }

    private fun receiveLoop() {
        val buf = ByteArray(512)
        val incoming = DatagramPacket(buf, buf.size)
        while (running) {
            try {
                socket.receive(incoming)
            } catch (_: SocketTimeoutException) {
                continue
            } catch (_: IOException) {
                break
            }
            val from = incoming.socketAddress as? InetSocketAddress ?: continue
            if (from in candidates) handle(buf, incoming.length, from)
        }
    }

    private fun handle(buf: ByteArray, len: Int, from: InetSocketAddress) {
        when (PacketReader(buf, len).header()) {
            Proto.WELCOME -> {
                if (sessionKey != null) return
                val (body, mac) = Proto.splitMac(buf, len) ?: return
                val r = PacketReader(body)
                r.header()
                val hnonce = r.bytes(Proto.NONCE_LEN) ?: return
                val key = Crypto.sessionKey(host.key, cnonce, hnonce)
                if (!Crypto.verify16(key, body, mac)) return
                lastStatusSeq = 0
                reportedPlayer = -1
                hostAddress = from
                sessionKey = key
                connectSignal.offer(Unit)
                remember(from)
            }
            Proto.CONNECT_REJECT -> {
                // Unauthenticated, so only believed while we're still waiting to connect, and
                // only once every address we tried says so: an old one may now be another PC.
                if (sessionKey == null) {
                    rejectedBy += from
                    if (rejectedBy.containsAll(candidates)) {
                        rejected = true
                        connectSignal.offer(Unit)
                    }
                }
            }
            Proto.STATUS -> {
                if (from != hostAddress) return
                val key = sessionKey ?: return
                val (body, mac) = Proto.splitMac(buf, len) ?: return
                if (!Crypto.verify16(key, body, mac)) return
                val r = PacketReader(body)
                r.header()
                val statusSeq = r.u32() ?: return
                val large = r.u8() ?: return
                val small = r.u8() ?: return
                val player = r.u8() ?: return
                if (statusSeq <= lastStatusSeq) return
                lastStatusSeq = statusSeq
                lastStatusAt = SystemClock.elapsedRealtime()
                if (player != reportedPlayer) {
                    reportedPlayer = player
                    listener.onState(State.CONNECTED, player)
                }
                listener.onRumble(large, small)
            }
        }
    }

    /** Saves where the host is, now that it has proven itself at that address. */
    private fun remember(at: InetSocketAddress) {
        val ip = at.address.hostAddress ?: return
        // Discovery's name for the host, if that reply came from this same address.
        val name = Discovery.lastSeen[host.idHex]?.takeIf { it.address == ip && it.port == at.port }?.name ?: host.name
        if (ip != host.address || at.port != host.port || name != host.name) {
            store.updateAddress(host.idHex, ip, at.port, name)
        }
    }

    private fun resolve(address: String, port: Int): InetSocketAddress? =
        runCatching { InetSocketAddress(InetAddress.getByName(address), port) }.getOrNull()

    private companion object {
        const val HEARTBEAT_MS = 50L
        const val MIN_INTERVAL_MS = 4L
        const val CONNECT_RETRY_MS = 500L
        const val LOST_AFTER_MS = 3000L
    }
}
