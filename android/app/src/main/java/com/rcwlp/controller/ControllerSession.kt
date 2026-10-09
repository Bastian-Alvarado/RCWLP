package com.rcwlp.controller

import android.os.SystemClock
import java.io.IOException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketTimeoutException
import java.nio.ByteBuffer
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

/**
 * Streams controller state to one paired host and plays its rumble.
 *
 * The sender thread connects, then sends the full pad state on every change plus a heartbeat
 * every [HEARTBEAT_MS]. Each packet carries the whole state, so a lost packet costs nothing.
 * The receiver thread verifies the host's WELCOME and STATUS packets.
 */
class ControllerSession(
    private val host: PairedHost,
    private val clientId: ByteArray,
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

    @Volatile private var running = false
    @Volatile private var pad = PadState()
    @Volatile private var motion = Motion()
    private val wake = ArrayBlockingQueue<Unit>(1)
    /** Wakes the sender while it waits for WELCOME or CONNECT_REJECT. */
    private val connectSignal = ArrayBlockingQueue<Unit>(1)
    private val socket = DatagramSocket()
    private lateinit var hostAddress: InetAddress

    // Set by the receiver, read by the sender.
    @Volatile private var cnonce = ByteArray(0)
    @Volatile private var sessionKey: ByteArray? = null
    @Volatile private var rejected = false
    @Volatile private var lastStatusAt = 0L

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
            hostAddress = InetAddress.getByName(host.address)
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
        cnonce = Crypto.random(Proto.NONCE_LEN)
        val packet = PacketWriter(Proto.CONNECT).bytes(clientId).bytes(cnonce).u8(padType).sign(host.key)
        while (running) {
            if (sessionKey != null) return true
            if (rejected) {
                listener.onState(State.NOT_PAIRED, 0)
                return false
            }
            send(packet)
            connectSignal.poll(CONNECT_RETRY_MS, TimeUnit.MILLISECONDS)
        }
        return false
    }

    private fun streamInput() {
        val key = sessionKey ?: return
        val mac = Crypto.newMac(key)
        val buf = ByteBuffer.allocate(Proto.INPUT_BODY_LEN + Proto.MAC_LEN)
        seq = 0
        lastStatusAt = SystemClock.elapsedRealtime()
        while (running) {
            val p = pad
            val m = motion
            seq++
            buf.clear()
            buf.put(Proto.MAGIC).put(Proto.VERSION.toByte()).put(Proto.INPUT.toByte())
                .put(clientId).putInt(seq)
                .putShort(p.buttons.toShort()).put(p.leftTrigger.toByte()).put(p.rightTrigger.toByte())
                .putShort(p.lx.toShort()).putShort(p.ly.toShort()).putShort(p.rx.toShort()).putShort(p.ry.toShort())
                .putShort(m.gx.toShort()).putShort(m.gy.toShort()).putShort(m.gz.toShort())
                .putShort(m.ax.toShort()).putShort(m.ay.toShort()).putShort(m.az.toShort())
            mac.update(buf.array(), 0, Proto.INPUT_BODY_LEN)
            buf.put(mac.doFinal(), 0, Proto.MAC_LEN)
            send(buf.array())
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
        val packet = PacketWriter(Proto.DISCONNECT).bytes(clientId).u32(seq + 1).sign(key)
        repeat(3) { runCatching { send(packet) } }
    }

    private fun send(data: ByteArray) {
        socket.send(DatagramPacket(data, data.size, hostAddress, host.port))
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
            if (incoming.address == hostAddress) handle(buf, incoming.length)
        }
    }

    private fun handle(buf: ByteArray, len: Int) {
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
                sessionKey = key
                connectSignal.offer(Unit)
            }
            Proto.CONNECT_REJECT -> {
                // Unauthenticated, so only believed while we're still waiting to connect.
                if (sessionKey == null) {
                    rejected = true
                    connectSignal.offer(Unit)
                }
            }
            Proto.STATUS -> {
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

    private companion object {
        const val HEARTBEAT_MS = 50L
        const val MIN_INTERVAL_MS = 4L
        const val CONNECT_RETRY_MS = 500L
        const val LOST_AFTER_MS = 3000L
    }
}
