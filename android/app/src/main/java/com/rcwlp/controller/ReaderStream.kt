package com.rcwlp.controller

import android.os.SystemClock
import android.util.Log
import java.io.IOException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.SocketTimeoutException
import java.nio.ByteBuffer
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicIntegerArray
import java.util.concurrent.atomic.AtomicLongArray
import kotlin.concurrent.thread

/**
 * Xbox mode in background mode: sends INPUT to the PC straight from the reader process, which
 * runs as the shell user. With the screen off, phone makers' power managers freeze apps (Honor
 * freezes RCWLP for a quarter second every three seconds), but not this process, so input keeps
 * flowing. The app still connects; it hands over the session key and gets each STATUS back.
 *
 * Every change is sent at once and repeated twice shortly after, and short presses are held for
 * [PressHold.MIN_PRESS_MS], so a lost packet on a busy Wi-Fi doesn't lose a tap. The host sends
 * STATUS to wherever INPUT comes from, which is this process's socket from the first packet on.
 */
class ReaderStream(
    private val sessionKey: ByteArray,
    private val clientId: ByteArray,
    private val host: InetSocketAddress,
    firstSeq: Int,
    private val tuning: Tuning,
    /** A verified STATUS and the round trip in ms (-1 before the first measurement). */
    private val onStatus: (Status, Int) -> Unit,
) {
    private val socket = DatagramSocket()
    @Volatile private var running = true
    private val wake = ArrayBlockingQueue<Unit>(1)

    // Guarded by `this`.
    private val pads = sortedMapOf(0 to PadState())
    private val holds = HashMap<Int, PressHold>()
    private var repeatsLeft = 0

    @Volatile var battery = Proto.BATTERY_UNKNOWN

    // Sender thread only, until stop() reads it after the thread ends.
    private var seq = firstSeq
    private lateinit var sender: Thread

    private val sentAt = AtomicLongArray(LATENCY_SLOTS)
    private val sentSeq = AtomicIntegerArray(LATENCY_SLOTS)
    @Volatile private var rttMs = -1
    private var lastStatusSeq = 0L
    @Volatile private var lastStatusAt = SystemClock.elapsedRealtime()

    /** How long since the host last sent a STATUS. */
    val msSinceStatus get() = SystemClock.elapsedRealtime() - lastStatusAt

    fun start() {
        socket.soTimeout = 200
        sender = thread(name = "reader-send") { sendLoop() }
        thread(name = "reader-recv") { receiveLoop() }
    }

    /** Stops sending and returns the last seq sent. */
    fun stop(): Int {
        running = false
        wake.offer(Unit)
        sender.join(500)
        socket.close()
        return seq
    }

    fun update(slot: Int, raw: PadState) {
        val now = SystemClock.uptimeMillis()
        synchronized(this) {
            pads[slot] = tuning.apply(raw)
            holds.getOrPut(slot) { PressHold() }.update(raw.buttons, now)
            repeatsLeft = REPEATS
        }
        wake.offer(Unit)
    }

    /** A controller was detached. Slot 0 stays, neutral, so the PC keeps its first controller. */
    fun remove(slot: Int) {
        synchronized(this) {
            if (slot == 0) pads[0] = PadState() else pads.remove(slot)
            holds.remove(slot)
            repeatsLeft = REPEATS
        }
        wake.offer(Unit)
    }

    private fun sendLoop() {
        val mac = Crypto.newMac(sessionKey)
        val buf = ByteBuffer.allocate(Proto.INPUT_MAX_LEN)
        try {
            while (running) {
                val now = SystemClock.uptimeMillis()
                val wait: Long
                val reports = synchronized(this) {
                    val r = pads.map { (slot, state) ->
                        val buttons = holds[slot]?.buttons(now) ?: state.buttons
                        PadReport(slot, state.copy(buttons = buttons))
                    }
                    // Wake again for a repeat, for a held press to end, or for the heartbeat.
                    val nextRelease = holds.values.mapNotNull { it.nextRelease(now) }.minOrNull()
                    var next = HEARTBEAT_MS
                    if (repeatsLeft > 0) {
                        repeatsLeft--
                        next = minOf(next, REPEAT_MS)
                    }
                    if (nextRelease != null) next = minOf(next, (nextRelease - now).coerceAtLeast(1))
                    wait = next
                    r
                }
                seq++
                val len = Packets.input(buf, mac, clientId, seq, if (rttMs < 0) Proto.RTT_UNKNOWN else rttMs, battery, reports)
                val i = seq and (LATENCY_SLOTS - 1)
                sentAt.set(i, SystemClock.elapsedRealtime())
                sentSeq.set(i, seq)
                socket.send(DatagramPacket(buf.array(), len, host))

                val sentTime = SystemClock.uptimeMillis()
                wake.poll(wait, TimeUnit.MILLISECONDS)
                // Batch bursts of changes into at most one packet per interval.
                val gap = MIN_INTERVAL_MS - (SystemClock.uptimeMillis() - sentTime)
                if (gap > 0) Thread.sleep(gap)
            }
        } catch (e: IOException) {
            if (running) Log.w(TAG, "Sending stopped", e)
        } catch (_: InterruptedException) {
        }
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
            if (incoming.socketAddress != host) continue
            val len = incoming.length
            if (PacketReader(buf, len).header() != Proto.STATUS) continue
            val (body, mac) = Proto.splitMac(buf, len) ?: continue
            if (!Crypto.verify16(sessionKey, body, mac)) continue
            val r = PacketReader(body)
            r.header()
            val status = Status.read(r) ?: continue
            if (status.seq <= lastStatusSeq) continue
            lastStatusSeq = status.seq
            lastStatusAt = SystemClock.elapsedRealtime()
            measure(status)
            runCatching { onStatus(status, rttMs) }
        }
    }

    /** Round trip to the acknowledged INPUT, minus the time the host held it; smoothed. */
    private fun measure(status: Status) {
        val i = status.ackSeq.toInt() and (LATENCY_SLOTS - 1)
        if (sentSeq.get(i).toLong() != status.ackSeq) return
        val sample = (SystemClock.elapsedRealtime() - sentAt.get(i) - status.ackAgeMs).coerceAtLeast(0).toInt()
        rttMs = if (rttMs < 0) sample else (rttMs * 3 + sample + 2) / 4
    }

    private companion object {
        const val TAG = "RCWLP-stream"
        const val HEARTBEAT_MS = 50L
        const val MIN_INTERVAL_MS = 4L
        /** A change goes out at once and twice more this far apart, in case one is lost. */
        const val REPEAT_MS = 8L
        const val REPEATS = 2
        const val LATENCY_SLOTS = 256
    }
}
