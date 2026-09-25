package online.vegameta.botato

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.text.SpannableString
import android.text.style.ForegroundColorSpan
import androidx.core.app.NotificationCompat
import androidx.core.app.Person
import androidx.core.content.ContextCompat

/**
 * Keeps an active call alive in the background (microphone, camera, screen capture need a
 * foreground service of the matching type) and shows a non-dismissible CallStyle notification
 * with mute and hang-up actions.
 */
class CallForegroundService : Service() {
    data class State(
        val peer: String = "",
        val status: String = "",
        val muted: Boolean = false,
        val camera: Boolean = false,
        val screen: Boolean = false,
    )

    companion object {
        private const val CHANNEL_ID = "calls"
        private const val NOTIFICATION_ID = 1
        private const val ACTION_MUTE = "online.vegameta.botato.MUTE"
        private const val ACTION_HANGUP = "online.vegameta.botato.HANGUP"
        private const val UNMUTED = 0xFF2EB3DC.toInt() // potato
        private const val MUTED = 0xFF7C3AED.toInt() // ember

        /** Delivers "mute" / "hangup" to Dart. */
        var onAction: ((String) -> Unit)? = null

        @Volatile
        var state = State()
        @Volatile
        private var running = false

        fun show(ctx: Context, s: State) {
            state = s
            ContextCompat.startForegroundService(ctx, Intent(ctx, CallForegroundService::class.java))
        }

        /** Called (on the main thread) once startForeground has applied the latest state. */
        private val applied = mutableListOf<() -> Unit>()

        /**
         * Starting a foreground service is asynchronous; [onApplied] runs only after the new
         * types are in effect, so MediaProjection can be created right after it.
         */
        fun setTypes(ctx: Context, camera: Boolean, screen: Boolean, onApplied: () -> Unit) {
            state = state.copy(camera = camera, screen = screen)
            if (!running) {
                onApplied()
                return
            }
            synchronized(applied) { applied.add(onApplied) }
            show(ctx, state)
        }

        private fun flushApplied() {
            val callbacks = synchronized(applied) { applied.toList().also { applied.clear() } }
            callbacks.forEach { it() }
        }

        fun hide(ctx: Context) {
            ctx.stopService(Intent(ctx, CallForegroundService::class.java))
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_MUTE -> onAction?.invoke("mute")
            ACTION_HANGUP -> onAction?.invoke("hangup")
        }
        running = true
        startInForeground()
        flushApplied()
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        running = false
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    private fun granted(permission: String) =
        ContextCompat.checkSelfPermission(this, permission) == PackageManager.PERMISSION_GRANTED

    private fun startInForeground() {
        val notification = buildNotification()
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION_ID, notification)
            return
        }
        // Each type is allowed only while its permission / consent is in place.
        var types = 0
        if (granted(Manifest.permission.RECORD_AUDIO)) types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
        if (state.camera && granted(Manifest.permission.CAMERA)) types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA
        if (state.screen) types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION
        try {
            startForeground(NOTIFICATION_ID, notification, types)
        } catch (e: SecurityException) {
            // E.g. projection consent revoked: keep at least the microphone.
            startForeground(NOTIFICATION_ID, notification, types and ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION.inv())
        }
    }

    private fun actionIntent(action: String, code: Int): PendingIntent =
        PendingIntent.getService(
            this,
            code,
            Intent(this, CallForegroundService::class.java).setAction(action),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    private fun buildNotification(): android.app.Notification {
        val manager = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            // Every time: for an existing channel this only renames it (the UI language may change).
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL_ID, Lang.tr(this, "Звонки", "Calls"), NotificationManager.IMPORTANCE_LOW).apply {
                    description = Lang.tr(this@CallForegroundService, "Идущий звонок", "Ongoing call")
                    setShowBadge(false)
                },
            )
        }
        val s = state
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val person = Person.Builder().setName(s.peer.ifEmpty { "botato" }).setImportant(true).build()
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_call)
            .setContentTitle(s.peer)
            .setContentText(s.status)
            .setContentIntent(open)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setStyle(NotificationCompat.CallStyle.forOngoingCall(person, actionIntent(ACTION_HANGUP, 2)))
            .addAction(
                if (s.muted) R.drawable.ic_call_mic_off else R.drawable.ic_call_mic,
                filled(if (s.muted) Lang.tr(this, "Включить микрофон", "Unmute") else Lang.tr(this, "Выключить микрофон", "Mute"), if (s.muted) MUTED else UNMUTED),
                actionIntent(ACTION_MUTE, 1),
            )
            .build()
    }

    /**
     * A title colored over its full length: CallStyle then fills the button with that color and
     * picks a contrasting text color (the default fill is unreadable on some skins, e.g. One UI).
     */
    private fun filled(title: String, color: Int): CharSequence =
        SpannableString(title).apply { setSpan(ForegroundColorSpan(color), 0, length, 0) }
}
