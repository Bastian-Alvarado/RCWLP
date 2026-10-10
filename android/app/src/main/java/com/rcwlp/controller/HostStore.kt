package com.rcwlp.controller

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

class PairedHost(val idHex: String, val name: String, val key: ByteArray, val address: String, val port: Int)

/** A host that answered discovery; may or may not be paired. */
class FoundHost(
    val idHex: String,
    val name: String,
    val address: String,
    val port: Int,
    /** The protocol version it answered in. */
    val protocol: Int = Proto.VERSION,
    /** The host program's version, from 0.2.0 on. */
    val hostVersion: String? = null,
) {
    val compatible get() = protocol == Proto.VERSION

    /** Why this app can't use the host, or null if it can. */
    fun incompatibility(): String? = when {
        compatible -> null
        protocol < Proto.VERSION -> "This PC runs an older RCWLP host${hostVersion?.let { " ($it)" }.orEmpty()}. Update it on the PC to connect."
        else -> "This PC runs a newer RCWLP host${hostVersion?.let { " ($it)" }.orEmpty()}. Update this app to connect."
    }
}

/** This phone's identity and the PCs it is paired with. */
class HostStore(context: Context) {
    private val prefs = context.getSharedPreferences("rcwlp", Context.MODE_PRIVATE)

    val clientId: ByteArray = prefs.getString("client_id", null)?.hexToBytes()
        ?: Crypto.random(Proto.ID_LEN).also { prefs.edit().putString("client_id", it.toHex()).apply() }

    /** Emulate a DualShock 4 with the phone's motion sensors instead of an Xbox 360 pad. */
    var ps4Mode: Boolean
        get() = prefs.getBoolean("ps4_mode", false)
        set(value) = prefs.edit().putBoolean("ps4_mode", value).apply()

    /** Keep working while locked or in other apps, reading the controller through Shizuku. */
    var backgroundMode: Boolean
        get() = prefs.getBoolean("background_mode", false)
        set(value) = prefs.edit().putBoolean("background_mode", value).apply()

    fun hosts(): List<PairedHost> {
        val arr = JSONArray(prefs.getString("hosts", "[]"))
        return (0 until arr.length()).map {
            val o = arr.getJSONObject(it)
            PairedHost(o.getString("id"), o.getString("name"), o.getString("key").hexToBytes(), o.getString("address"), o.getInt("port"))
        }
    }

    fun find(idHex: String): PairedHost? = hosts().firstOrNull { it.idHex == idHex }

    @Synchronized
    fun save(host: PairedHost) = write(hosts().filter { it.idHex != host.idHex } + host)

    @Synchronized
    fun remove(idHex: String) = write(hosts().filter { it.idHex != idHex })

    /** Records where a paired host turned out to be. Does nothing if it was forgotten meanwhile. */
    @Synchronized
    fun updateAddress(idHex: String, address: String, port: Int, name: String) {
        val hosts = hosts()
        if (hosts.none { it.idHex == idHex }) return
        write(hosts.map { if (it.idHex == idHex) PairedHost(it.idHex, name, it.key, address, port) else it })
    }

    private fun write(hosts: List<PairedHost>) {
        val arr = JSONArray()
        hosts.forEach {
            arr.put(
                JSONObject()
                    .put("id", it.idHex)
                    .put("name", it.name)
                    .put("key", it.key.toHex())
                    .put("address", it.address)
                    .put("port", it.port),
            )
        }
        prefs.edit().putString("hosts", arr.toString()).apply()
    }
}
