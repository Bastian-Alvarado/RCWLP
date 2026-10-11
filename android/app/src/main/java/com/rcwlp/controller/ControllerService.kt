package com.rcwlp.controller

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import rikka.shizuku.Shizuku
import java.util.concurrent.CopyOnWriteArraySet

/**
 * Background mode: keeps a phone connected as a controller while it's locked or in other apps.
 * Controller input comes from [EvdevService] (raw /dev/input, via Shizuku) instead of an
 * activity, and a partial wake lock plus Wi-Fi locks keep the link fast with the screen off.
 */
class ControllerService : Service(), ControllerSession.Listener {
    data class Status(
        val active: Boolean = false,
        val hostId: String? = null,
        val hostName: String = "",
        val message: String = "",
        val controllers: String = "",
    )

    private lateinit var store: HostStore
    private val handler = Handler(Looper.getMainLooper())
    @Volatile private var stateText = ""
    @Volatile private var connected = false
    private val refreshStatus = object : Runnable {
        override fun run() {
            publishState()
            handler.postDelayed(this, STATUS_REFRESH_MS)
        }
    }
    /** Rumble per controller slot; only slot 0 may buzz the phone itself. */
    private val rumblers = java.util.concurrent.ConcurrentHashMap<Int, Rumbler>()
    private var host: PairedHost? = null
    private var session: ControllerSession? = null
    private var motion: MotionSource? = null
    private var ps4Mode = false
    private var readerBound = false
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLocks: List<WifiManager.WifiLock> = emptyList()

    private val readerArgs by lazy {
        Shizuku.UserServiceArgs(ComponentName(packageName, EvdevService::class.java.name))
            .tag("evdev")
            .daemon(false) // Dies with the app, which also releases the controller grab.
            .processNameSuffix("evdev")
            .debuggable(BuildConfig.DEBUG)
            // Any reinstall changes this, so Shizuku never keeps running an older reader.
            .version((packageManager.getPackageInfo(packageName, 0).lastUpdateTime / 1000).toInt())
    }

    private val readerCallback = object : IEvdevCallback.Stub() {
        override fun onState(slot: Int, buttons: Int, leftTrigger: Int, rightTrigger: Int, lx: Int, ly: Int, rx: Int, ry: Int) {
            session?.update(slot, PadState(buttons, leftTrigger, rightTrigger, lx, ly, rx, ry))
        }

        override fun onRemoved(slot: Int) {
            rumblers.remove(slot)?.stop()
            session?.remove(slot)
        }

        override fun onDevices(names: String) {
            // Grabbed controllers still show up as Android input devices, which is how their
            // motors are reached. Match them to slots by order, which is how slots were handed out.
            val devices = GamepadMapper.connected()
            val lines = names.lineSequence().filter { it.isNotBlank() }.toList()
            for (line in lines) {
                val slot = line.substringBefore(':').toIntOrNull() ?: continue
                rumblers.getOrPut(slot) { Rumbler(this@ControllerService, phoneFallback = slot == 0) }.device = devices.getOrNull(slot)
            }
            publish(status.copy(controllers = lines.joinToString("\n") { it.substringAfter(':') }))
        }
    }

    private val readerConnection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName, binder: IBinder?) {
            val reader = binder?.takeIf { it.pingBinder() }?.let { IEvdevService.Stub.asInterface(it) }
            if (reader == null) {
                fail("Couldn't start the controller reader through Shizuku.")
                return
            }
            runCatching { reader.start(readerCallback) }.onFailure { fail("Controller reader failed: ${it.message}") }
        }

        override fun onServiceDisconnected(name: ComponentName) {
            if (session != null) fail("The controller reader stopped. Is Shizuku still running?")
        }
    }

    private val shizukuDied = Shizuku.OnBinderDeadListener {
        fail("Shizuku stopped. Start it again from the Shizuku app.")
    }

    override fun onCreate() {
        super.onCreate()
        store = HostStore(this)
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopSelf()
            return START_NOT_STICKY
        }
        val target = intent?.getStringExtra(EXTRA_HOST_ID)?.let(store::find)
        if (target == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        enterForeground(target.name, "Connecting…")
        if (session != null && host?.idHex == target.idHex) return START_NOT_STICKY

        teardown()
        host = target
        publish(Status(active = true, hostId = target.idHex, hostName = target.name, message = "Connecting to PC…"))
        setup(target)
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        teardown()
        publish(status.copy(active = false))
        super.onDestroy()
    }

    private fun setup(target: PairedHost) {
        acquireLocks()
        ps4Mode = store.ps4Mode
        val padType = if (ps4Mode) Proto.PAD_DUALSHOCK4 else Proto.PAD_XBOX360
        // Slot 0 can always fall back to the phone's own vibration, even before a controller appears.
        rumblers[0] = Rumbler(this)
        val s = ControllerSession(store, target, padType, this, { batteryLevel(this) }).also { it.start() }
        session = s
        if (ps4Mode) motion = MotionSource(this, orientFromGravity = true) { s.update(it) }.also { it.start() }
        bindReader()
        handler.post(refreshStatus)
    }

    private fun teardown() {
        handler.removeCallbacks(refreshStatus)
        connected = false
        if (readerBound) {
            // remove = true makes Shizuku call EvdevService.destroy(), ending its process and the grab.
            runCatching { Shizuku.unbindUserService(readerArgs, readerConnection, true) }
            runCatching { Shizuku.removeBinderDeadListener(shizukuDied) }
            readerBound = false
        }
        motion?.stop()
        motion = null
        session?.stop()
        session = null
        rumblers.values.forEach(Rumbler::stop)
        rumblers.clear()
        releaseLocks()
    }

    private fun bindReader() {
        when (ShizukuState.current(this)) {
            ShizukuState.READY -> {}
            ShizukuState.NO_PERMISSION -> return fail("RCWLP isn't allowed in Shizuku yet. Open RCWLP to allow it.")
            else -> return fail("Shizuku isn't running. Start it from the Shizuku app, then reconnect.")
        }
        try {
            Shizuku.bindUserService(readerArgs, readerConnection)
            Shizuku.addBinderDeadListener(shizukuDied)
            readerBound = true
        } catch (e: RuntimeException) {
            fail("Couldn't reach Shizuku: ${e.message}")
        }
    }

    /** Shows why background mode stopped, then stops. */
    private fun fail(message: String) {
        Log.w(TAG, message)
        publish(status.copy(active = false, message = message))
        getSystemService(NotificationManager::class.java)
            .notify(ERROR_NOTIFICATION_ID, notification(host?.name ?: "RCWLP", message, ongoing = false))
        stopSelf()
    }

    override fun onState(state: ControllerSession.State, detail: Int) {
        val target = host ?: return
        val message = ControllerSession.describe(state, detail, target, ps4Mode)
        stateText = message
        connected = state == ControllerSession.State.CONNECTED
        publishState()
        if (state == ControllerSession.State.NOT_PAIRED) fail(message) else host?.let { enterForeground(it.name, message) }
    }

    /** The in-app status, with the round trip while connected. The notification keeps the plain state. */
    private fun publishState() {
        val latency = session?.latencyMs?.takeIf { connected }
        publish(status.copy(message = if (latency != null) "$stateText · $latency ms" else stateText))
    }

    override fun onFeedback(pads: List<Feedback>) {
        if (pads.isEmpty()) rumblers.values.forEach(Rumbler::stop)
        for (f in pads) rumblers[f.slot]?.rumble(f.large, f.small)
    }

    private fun enterForeground(title: String, text: String) {
        val n = notification(title, text, ongoing = true)
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIFICATION_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        } else {
            startForeground(NOTIFICATION_ID, n)
        }
    }

    private fun notification(title: String, text: String, ongoing: Boolean): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        if (manager.getNotificationChannel(CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL, "Controller connection", NotificationManager.IMPORTANCE_LOW),
            )
        }
        val open = PendingIntent.getActivity(
            this, 0,
            Intent(this, ControllerActivity::class.java)
                .putExtra(ControllerActivity.EXTRA_HOST_ID, host?.idHex)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val builder = Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_controller)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(open)
            .setOngoing(ongoing)
            .setOnlyAlertOnce(true)
            .setCategory(Notification.CATEGORY_SERVICE)
        if (ongoing) {
            val stop = PendingIntent.getService(
                this, 1, Intent(this, ControllerService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE,
            )
            builder.addAction(Notification.Action.Builder(null, "Disconnect", stop).build())
        }
        if (Build.VERSION.SDK_INT >= 31) builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        return builder.build()
    }

    @Suppress("DEPRECATION", "WakelockTimeout")
    private fun acquireLocks() {
        wakeLock = getSystemService(PowerManager::class.java)
            .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "rcwlp:controller")
            .apply { acquire() }
        val wifi = applicationContext.getSystemService(WifiManager::class.java)
        // Low latency only applies while the screen is on. High-perf keeps Wi-Fi power saving off
        // when it's not, but it's deprecated since Android 14 and may do nothing there, so screen-off
        // rumble can arrive later. Input isn't affected much: the phone sends at least every 50 ms.
        wifiLocks = listOfNotNull(
            if (Build.VERSION.SDK_INT >= 29) wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "rcwlp:low-latency") else null,
            wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "rcwlp:high-perf"),
        ).onEach {
            it.setReferenceCounted(false)
            it.acquire()
        }
    }

    private fun releaseLocks() {
        wifiLocks.forEach { if (it.isHeld) it.release() }
        wifiLocks = emptyList()
        wakeLock?.takeIf { it.isHeld }?.release()
        wakeLock = null
    }

    companion object {
        private const val TAG = "RCWLP"
        private const val ACTION_STOP = "com.rcwlp.controller.STOP"
        private const val EXTRA_HOST_ID = "host_id"
        private const val CHANNEL = "controller"
        private const val NOTIFICATION_ID = 1
        private const val ERROR_NOTIFICATION_ID = 2
        private const val STATUS_REFRESH_MS = 1_000L

        @Volatile
        var status = Status()
            private set
        private val observers = CopyOnWriteArraySet<(Status) -> Unit>()

        /** [observer] runs right away and on every change, on any thread. */
        fun observe(observer: (Status) -> Unit) {
            observers += observer
            observer(status)
        }

        fun unobserve(observer: (Status) -> Unit) {
            observers -= observer
        }

        private fun publish(s: Status) {
            status = s
            observers.forEach { it(s) }
        }

        fun start(context: Context, hostId: String) {
            context.getSystemService(NotificationManager::class.java).cancel(ERROR_NOTIFICATION_ID)
            context.startForegroundService(Intent(context, ControllerService::class.java).putExtra(EXTRA_HOST_ID, hostId))
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, ControllerService::class.java))
        }
    }
}

/** Where Shizuku setup stands, for background mode. */
enum class ShizukuState {
    NOT_INSTALLED, NOT_RUNNING, NO_PERMISSION, READY;

    companion object {
        const val PACKAGE = "moe.shizuku.privileged.api"

        fun current(context: Context): ShizukuState {
            val running = runCatching { Shizuku.pingBinder() && !Shizuku.isPreV11() }.getOrDefault(false)
            if (!running) {
                val installed = runCatching { context.packageManager.getPackageInfo(PACKAGE, 0) }.isSuccess
                // Sui (root) has no app to find but still answers pingBinder, so only this branch checks.
                return if (installed) NOT_RUNNING else NOT_INSTALLED
            }
            val granted = runCatching { Shizuku.checkSelfPermission() == PackageManager.PERMISSION_GRANTED }.getOrDefault(false)
            return if (granted) READY else NO_PERMISSION
        }
    }
}
