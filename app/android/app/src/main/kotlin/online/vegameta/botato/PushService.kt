package online.vegameta.botato

import android.os.Handler
import android.os.Looper
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage

/**
 * FCM pushes carry nothing: they only mean "connect to the relay". A running app just
 * reconnects (its socket may have died in Doze); otherwise the app starts headless and
 * fetches calls and messages itself.
 */
class PushService : FirebaseMessagingService() {
    override fun onMessageReceived(message: RemoteMessage) {
        Handler(Looper.getMainLooper()).post {
            if (BotatoApp.hasEngine()) {
                CallChannel.invoke("wake", null)
            } else {
                BotatoApp.engine(applicationContext)
            }
        }
    }

    override fun onNewToken(token: String) {
        CallChannel.invoke("pushToken", token)
    }
}
