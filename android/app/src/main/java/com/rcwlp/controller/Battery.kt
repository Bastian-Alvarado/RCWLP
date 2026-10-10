package com.rcwlp.controller

import android.content.Context
import android.os.BatteryManager

/** The phone's battery in INPUT's encoding: bit 7 while charging, bits 0–6 the percentage. */
fun batteryLevel(context: Context): Int {
    val manager = context.getSystemService(BatteryManager::class.java) ?: return Proto.BATTERY_UNKNOWN
    val percent = manager.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)
    if (percent !in 0..100) return Proto.BATTERY_UNKNOWN
    return percent or (if (manager.isCharging) Proto.BATTERY_CHARGING else 0)
}
