package online.vegameta.botato

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.Person

/**
 * Ringing incoming-call notification: accept / decline, full-screen over the lock screen.
 * Accepting from here answers without camera. Works while the app process is alive;
 * waking a killed app needs push (later).
 */
object IncomingCallNotification {
    // v2: our own ring (res/raw/ring); a channel's sound can't change after creation.
    private const val CHANNEL_ID = "incoming_calls_v2"
    private const val OLD_CHANNEL_ID = "incoming_calls"
    private const val NOTIFICATION_ID = 2
    private const val RING_TIMEOUT_MS = 45_000L
    const val ACTION_ANSWER = "online.vegameta.botato.ANSWER"
    const val ACTION_SHOW = "online.vegameta.botato.SHOW_INCOMING"
    const val ACTION_DECLINE = "online.vegameta.botato.DECLINE"

    fun show(ctx: Context, peer: String, video: Boolean) {
        val manager = ctx.getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && manager.getNotificationChannel(CHANNEL_ID) == null) {
            manager.deleteNotificationChannel(OLD_CHANNEL_ID)
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL_ID, "Входящие звонки", NotificationManager.IMPORTANCE_HIGH).apply {
                    setSound(
                        Sounds.uri(ctx, "ring"),
                        AudioAttributes.Builder()
                            .setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
                            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                            .build(),
                    )
                    enableVibration(true)
                    vibrationPattern = longArrayOf(0, 800, 800)
                    lockscreenVisibility = Notification.VISIBILITY_PUBLIC
                },
            )
        }
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        fun activity(action: String, code: Int) = PendingIntent.getActivity(
            ctx,
            code,
            IntentGuard.sign(Intent(ctx, MainActivity::class.java))
                .setAction(action)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            flags,
        )
        val decline = PendingIntent.getBroadcast(
            ctx,
            12,
            Intent(ctx, CallActionReceiver::class.java).setAction(ACTION_DECLINE),
            flags,
        )
        val person = Person.Builder().setName(peer).setImportant(true).build()
        val n = NotificationCompat.Builder(ctx, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_call)
            .setContentTitle(peer)
            .setContentText(if (video) "Входящий видеозвонок" else "Входящий звонок")
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setPriority(NotificationCompat.PRIORITY_MAX)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .setAutoCancel(false)
            .setTimeoutAfter(RING_TIMEOUT_MS)
            .setContentIntent(activity(ACTION_SHOW, 10))
            .setFullScreenIntent(activity(ACTION_SHOW, 11), true)
            .setStyle(NotificationCompat.CallStyle.forIncomingCall(person, decline, activity(ACTION_ANSWER, 13)))
            .build()
        n.flags = n.flags or Notification.FLAG_INSISTENT // ring until answered
        manager.notify(NOTIFICATION_ID, n)
    }

    fun cancel(ctx: Context) {
        ctx.getSystemService(NotificationManager::class.java).cancel(NOTIFICATION_ID)
    }
}

/** Decline from the notification without opening the app. */
class CallActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == IncomingCallNotification.ACTION_DECLINE) {
            IncomingCallNotification.cancel(context)
            CallForegroundService.onAction?.invoke("decline")
        }
    }
}
