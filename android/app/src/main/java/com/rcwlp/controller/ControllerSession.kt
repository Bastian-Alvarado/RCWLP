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
import java.util.concurrent.atomic.AtomicIntegerArray
import java.util.concurrent.atomic.AtomicLongArray
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
    /** The phone's battery in INPUT's encoding; called every few seconds on the network thread. */
    private val battery: () -> Int = { Proto.BATTERY_UNKNOWN },
) {
    enum class State {
        CONNECTING,
        /** Still trying, but the host hasn't answered for a few seconds. */
        NO_ANSWER,
        /** The host answered discovery in another protocol version, passed as `detail`. */
        WRONG_VERSION,
        CONNECTED,
        NOT_PAIRED,
    }

    /** Called from network threads. */
    interface Listener {
        /** `detail` is the player number for [State.CONNECTED] and the host's protocol version for [State.WRONG_VERSION]. */
        fun onState(state: State, detail: Int)
        /** Rumble, player and lightbar per controller slot. Empty means stop everything. */
        fun onFeedback(pads: List<Feedback>)
    }

    private val clientId = store.clientId
    @Volatile private var running = false
    /** Controller state by slot. Slot 0 is always sent, so the PC gets a controller right away. */
    @Volatile private var pads: Map<Int, PadState> = mapOf(0 to PadState())
    @Volatile private var motion = Motion()
    @Volatile private var touch = Touch()

    /** Smoothed round trip to the host in ms, or null before the first measurement. */
    @Volatile var latencyMs: Int? = null
        private set
    /** When each recent INPUT went out, by seq, for measuring the round trip. */
    private val sentAt = AtomicLongArray(LATENCY_SLOTS)
    private val sentSeq = AtomicIntegerArray(LATENCY_SLOTS)
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
    private var reportedWrongVersion = false

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

    /** The controller in slot 0. */
    fun update(state: PadState) = update(0, state)

    fun update(slot: Int, state: PadState) {
        require(slot in 0 until Proto.MAX_PADS)
        synchronized(this) {
            if (pads[slot] == state) return
            pads = pads + (slot to state)
        }
        wake.offer(Unit)
    }

    /** A controller was detached. Slot 0 stays, neutral, so the PC keeps its first controller. */
    fun remove(slot: Int) {
        synchronized(this) {
            pads = if (slot == 0) pads + (0 to PadState()) else pads - slot
        }
        wake.offer(Unit)
    }

    /** Called from the sensor thread. */
    fun update(state: Motion) {
        motion = state
        wake.offer(Unit)
    }

    /** Fingers on the phone screen, used as the DualShock 4 touchpad. */
    fun update(state: Touch) {
        if (state == touch) return
        touch = state
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
        val started = SystemClock.elapsedRealtime()
        var reportedNoAnswer = false
        while (running) {
            if (sessionKey != null) return true
            if (rejected) {
                listener.onState(State.NOT_PAIRED, 0)
                return false
            }
            candidates.forEach { send(packet, it) }
            if (SystemClock.elapsedRealtime() - started > NO_ANSWER_AFTER_MS) {
                if (!reportedNoAnswer) listener.onState(State.NO_ANSWER, 0)
                reportedNoAnswer = true
                // A host of another protocol version ignores CONNECT but still answers DISCOVER.
                for (c in candidates) Discovery.requests.forEach { send(it, c) }
            }
            connectSignal.poll(CONNECT_RETRY_MS, TimeUnit.MILLISECONDS)
        }
        return false
    }

    private fun streamInput() {
        val key = sessionKey ?: return
        val target = hostAddress ?: return
        val mac = Crypto.newMac(key)
        val buf = ByteBuffer.allocate(Proto.INPUT_MAX_LEN)
        seq = 0
        for (i in 0 until LATENCY_SLOTS) sentSeq.set(i, 0)
        lastStatusAt = SystemClock.elapsedRealtime()
        var batteryLevel = Proto.BATTERY_UNKNOWN
        var batteryReadAt = 0L
        while (running) {
            seq++
            val now = SystemClock.elapsedRealtime()
            if (now - batteryReadAt > BATTERY_EVERY_MS) {
                batteryLevel = runCatching(battery).getOrDefault(Proto.BATTERY_UNKNOWN)
                batteryReadAt = now
            }
            val reports = pads.toSortedMap().map { (slot, state) ->
                if (slot == 0) PadReport(0, state, motion, touch) else PadReport(slot, state)
            }
            val rtt = latencyMs?.coerceAtMost(Proto.RTT_UNKNOWN - 1) ?: Proto.RTT_UNKNOWN
            val len = Packets.input(buf, mac, clientId, seq, rtt, batteryLevel, reports)
            val i = seq and (LATENCY_SLOTS - 1)
            val sentTime = SystemClock.elapsedRealtime()
            sentAt.set(i, sentTime)
            sentSeq.set(i, seq)
            socket.send(DatagramPacket(buf.array(), len, target))

            if (sentTime - lastStatusAt > LOST_AFTER_MS) {
                latencyMs = null
                listener.onFeedback(emptyList())
                return // Host went quiet: start a fresh session.
            }
            wake.poll(HEARTBEAT_MS, TimeUnit.MILLISECONDS)
            // Sensors fire hundreds of times a second; batch them into at most one packet per interval.
            val wait = MIN_INTERVAL_MS - (SystemClock.elapsedRealtime() - sentTime)
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
        val version = PacketReader(buf, len).headerAnyVersion()?.first ?: return
        if (version != Proto.VERSION) {
            if (sessionKey == null && !reportedWrongVersion && Discovery.parseReply(buf, len, "", 0) != null) {
                reportedWrongVersion = true
                listener.onState(State.WRONG_VERSION, version)
            }
            return
        }
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
                val status = Status.read(r) ?: return
                if (status.seq <= lastStatusSeq) return
                lastStatusSeq = status.seq
                val now = SystemClock.elapsedRealtime()
                lastStatusAt = now
                measureLatency(status, now)
                val player = status.pads.firstOrNull { it.slot == 0 }?.player ?: 0
                if (player != reportedPlayer) {
                    reportedPlayer = player
                    listener.onState(State.CONNECTED, player)
                }
                listener.onFeedback(status.pads)
            }
        }
    }

    /**
     * The round trip is from sending the acknowledged INPUT to now, minus the time the host
     * held it before sending this STATUS. Smoothed so one late packet doesn't jump the display.
     */
    private fun measureLatency(status: Status, now: Long) {
        val i = (status.ackSeq.toInt()) and (LATENCY_SLOTS - 1)
        if (sentSeq.get(i).toLong() != status.ackSeq) return // Too old, or from before a reconnect.
        val sample = (now - sentAt.get(i) - status.ackAgeMs).coerceAtLeast(0).toInt()
        latencyMs = latencyMs?.let { (it * 3 + sample + 2) / 4 } ?: sample
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

    companion object {
        /** What to tell the user about a state; see [Listener.onState] for `detail`. */
        fun describe(state: State, detail: Int, host: PairedHost, ps4Mode: Boolean): String = when (state) {
            State.CONNECTING -> "Connecting to PC…"
            State.NO_ANSWER -> "No answer from ${host.name} at ${host.address}. Is rcwlp-host running there, and is this phone on the same Wi-Fi?"
            State.WRONG_VERSION ->
                if (detail < Proto.VERSION) "${host.name} runs an older RCWLP host. Update it on the PC."
                else "${host.name} runs a newer RCWLP host. Update this app."
            State.CONNECTED -> when {
                ps4Mode -> "Connected · PS4 controller with gyro"
                detail > 0 -> "Connected · Player $detail"
                else -> "Connected"
            }
            State.NOT_PAIRED -> "This PC doesn't recognize this phone anymore. Pair again."
        }

        private const val HEARTBEAT_MS = 50L
        private const val MIN_INTERVAL_MS = 4L
        private const val CONNECT_RETRY_MS = 500L
        private const val LOST_AFTER_MS = 3000L
        private const val NO_ANSWER_AFTER_MS = 3000L
        private const val BATTERY_EVERY_MS = 5000L
        /** Remembered send times; a power of two, and far more than are ever in flight. */
        private const val LATENCY_SLOTS = 256
    }
}
