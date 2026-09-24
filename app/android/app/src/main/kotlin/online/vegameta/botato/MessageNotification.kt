package online.vegameta.botato

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.core.app.NotificationCompat

/** New-message notification, one per conversation; tapping it opens that chat. */
object MessageNotification {
    private const val CHANNEL_ID = "messages"
    const val ACTION_OPEN_CHAT = "online.vegameta.botato.OPEN_CHAT"
    const val EXTRA_PEER = "peer"

    private fun id(peer: String) = 1000 + (peer.hashCode() and 0xffff)

    fun show(ctx: Context, peer: String, name: String, text: String) {
        val manager = ctx.getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && manager.getNotificationChannel(CHANNEL_ID) == null) {
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL_ID, "Сообщения", NotificationManager.IMPORTANCE_HIGH),
            )
        }
        val open = PendingIntent.getActivity(
            ctx,
            id(peer),
            Intent(ctx, MainActivity::class.java)
                .setAction(ACTION_OPEN_CHAT)
                .putExtra(EXTRA_PEER, peer)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val n = NotificationCompat.Builder(ctx, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_stat_message)
            .setContentTitle(name)
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setAutoCancel(true)
            .setContentIntent(open)
            .build()
        manager.notify(id(peer), n)
    }

    fun cancel(ctx: Context, peer: String) {
        ctx.getSystemService(NotificationManager::class.java).cancel(id(peer))
    }
}
