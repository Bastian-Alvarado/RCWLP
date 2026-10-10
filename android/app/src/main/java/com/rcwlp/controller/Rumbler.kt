package com.rcwlp.controller

import android.content.Context
import android.os.Build
import android.os.CombinedVibration
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.view.InputDevice
import kotlin.math.max

/**
 * Plays the PC's rumble on the controller's motors if it has any, otherwise on the phone.
 * The host resends the motor state whenever it changes, so each effect just runs until replaced.
 */
class Rumbler(
    context: Context,
    /** Vibrate the phone when the controller has no motors. Only right for the controller clipped to it. */
    private val phoneFallback: Boolean = true,
) {
    private val phone: Vibrator? = if (Build.VERSION.SDK_INT >= 31) {
        context.getSystemService(VibratorManager::class.java)?.defaultVibrator
    } else {
        @Suppress("DEPRECATION")
        context.getSystemService(Vibrator::class.java)
    }

    @Volatile
    var device: InputDevice? = null

    private var current = 0 to 0

    // InputDevice.vibrator is deprecated, but it's the only API below Android 12.
    @Suppress("DEPRECATION")
    @Synchronized
    fun rumble(large: Int, small: Int) {
        if (current == large to small) return
        current = large to small
        stopAll()
        if (large == 0 && small == 0) return

        val dev = device
        if (Build.VERSION.SDK_INT >= 31 && dev != null) {
            val manager = dev.vibratorManager
            val ids = manager.vibratorIds
            if (ids.size >= 2) {
                // Dual-motor controller: heavy motor first, light motor second.
                val combined = CombinedVibration.startParallel()
                if (large > 0) combined.addVibrator(ids[0], effect(large, true))
                if (small > 0) combined.addVibrator(ids[1], effect(small, true))
                manager.vibrate(combined.combine())
                return
            }
            if (ids.size == 1) {
                manager.vibrate(CombinedVibration.createParallel(effect(max(large, small), true)))
                return
            }
        } else if (dev != null && dev.vibrator.hasVibrator()) {
            dev.vibrator.vibrate(effect(max(large, small), dev.vibrator.hasAmplitudeControl()))
            return
        }
        if (!phoneFallback) return
        phone?.takeIf { it.hasVibrator() }?.let { it.vibrate(effect(max(large, small), it.hasAmplitudeControl())) }
    }

    @Synchronized
    fun stop() {
        current = 0 to 0
        stopAll()
    }

    @Suppress("DEPRECATION")
    private fun stopAll() {
        val dev = device
        if (Build.VERSION.SDK_INT >= 31 && dev != null) {
            dev.vibratorManager.cancel()
        } else if (dev != null) {
            dev.vibrator.cancel()
        }
        if (phoneFallback) phone?.cancel()
    }

    private fun effect(amplitude: Int, amplitudeControl: Boolean): VibrationEffect =
        VibrationEffect.createOneShot(
            LONG_MS,
            if (amplitudeControl) amplitude.coerceIn(1, 255) else VibrationEffect.DEFAULT_AMPLITUDE,
        )

    private companion object {
        /** Long enough to outlast any effect; the host always sends the stop explicitly. */
        const val LONG_MS = 60_000L
    }
}
