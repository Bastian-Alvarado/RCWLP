package com.rcwlp.controller

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.os.Handler
import android.os.HandlerThread
import android.view.Surface
import kotlin.math.abs
import kotlin.math.roundToInt

/** Motion in DualShock 4 units and axes: gyro 16 per deg/s, accel 8192 per g (see PROTOCOL.md). */
data class Motion(
    val gx: Int = 0, val gy: Int = 0, val gz: Int = 0,
    val ax: Int = 0, val ay: Int = 0, val az: Int = 0,
)

/**
 * Reads the phone's gyroscope and accelerometer and reports them in controller axes.
 * With a clip-on controller the phone *is* the controller body, so its sensors are what
 * a DualShock 4's would be.
 */
class MotionSource(
    context: Context,
    /**
     * Work out landscape/portrait from gravity instead of the screen. Needed in background mode:
     * the lock screen is portrait-only, so the screen's rotation stops matching how the phone is held.
     */
    private val orientFromGravity: Boolean = false,
    private val onMotion: (Motion) -> Unit,
) : SensorEventListener {
    private val sensors = context.getSystemService(SensorManager::class.java)
    private val gyroSensor: Sensor? = sensors.getDefaultSensor(Sensor.TYPE_GYROSCOPE)
    private val accelSensor: Sensor? = sensors.getDefaultSensor(Sensor.TYPE_ACCELEROMETER)
    private var thread: HandlerThread? = null

    // Sensor thread only.
    private val gyro = FloatArray(3)
    private val accel = FloatArray(3)

    /** Current Surface.ROTATION_*; the phone axes are remapped for it. */
    @Volatile
    var rotation = Surface.ROTATION_90

    val hasGyro get() = gyroSensor != null

    fun start() {
        if (thread != null) return
        val t = HandlerThread("rcwlp-motion").also { it.start() }
        thread = t
        val handler = Handler(t.looper)
        gyroSensor?.let { sensors.registerListener(this, it, SAMPLE_US, handler) }
        accelSensor?.let { sensors.registerListener(this, it, SAMPLE_US, handler) }
    }

    fun stop() {
        sensors.unregisterListener(this)
        thread?.quitSafely()
        thread = null
    }

    override fun onSensorChanged(event: SensorEvent) {
        when (event.sensor.type) {
            Sensor.TYPE_GYROSCOPE -> event.values.copyInto(gyro, endIndex = 3)
            Sensor.TYPE_ACCELEROMETER -> {
                event.values.copyInto(accel, endIndex = 3)
                if (orientFromGravity) updateRotationFromGravity()
            }
            else -> return
        }
        val g = toController(gyro)
        val a = toController(accel)
        onMotion(
            Motion(
                gx = units(g[0] * GYRO_PER_RAD), gy = units(g[1] * GYRO_PER_RAD), gz = units(g[2] * GYRO_PER_RAD),
                ax = units(a[0] * ACCEL_PER_MS2), ay = units(a[1] * ACCEL_PER_MS2), az = units(a[2] * ACCEL_PER_MS2),
            ),
        )
    }

    override fun onAccuracyChanged(sensor: Sensor, accuracy: Int) {}

    /** Picks the rotation whose "down" matches gravity, with hysteresis so it doesn't flicker. */
    private fun updateRotationFromGravity() {
        val (x, y) = accel
        val newRotation = when {
            abs(x) > MIN_GRAVITY && abs(x) > 1.5f * abs(y) -> if (x > 0) Surface.ROTATION_90 else Surface.ROTATION_270
            abs(y) > MIN_GRAVITY && abs(y) > 1.5f * abs(x) -> if (y > 0) Surface.ROTATION_0 else Surface.ROTATION_180
            else -> return // Lying flat or in between: keep the last one.
        }
        rotation = newRotation
    }

    /**
     * Phone axes (X right and Y up in portrait, Z out of the screen) to controller axes
     * (X right, Y out of the face, Z toward the player, i.e. down the screen as you hold it).
     */
    private fun toController(v: FloatArray): FloatArray {
        val (x, y, z) = v
        return when (rotation) {
            Surface.ROTATION_0 -> floatArrayOf(x, z, -y)
            Surface.ROTATION_90 -> floatArrayOf(-y, z, -x)
            Surface.ROTATION_180 -> floatArrayOf(-x, z, y)
            else -> floatArrayOf(y, z, x)
        }
    }

    private fun units(v: Float) = v.roundToInt().coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt())

    private companion object {
        const val SAMPLE_US = 5_000 // 200 Hz
        const val MIN_GRAVITY = 3f // m/s², on the in-screen axes
        const val GYRO_PER_RAD = (180.0 / Math.PI * 16.0).toFloat()
        const val ACCEL_PER_MS2 = (8192.0 / 9.80665).toFloat()
    }
}
