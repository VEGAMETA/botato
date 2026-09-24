package online.vegameta.botato

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine

class MainActivity : FlutterActivity() {
    /** The shared engine: a call or relay session started by a push keeps running. */
    override fun provideFlutterEngine(context: Context): FlutterEngine = BotatoApp.engine(context)

    override fun shouldDestroyEngineWithHost(): Boolean = false

    override fun onCreate(savedInstanceState: Bundle?) {
        CallChannel.activity = this
        super.onCreate(savedInstanceState)
        handleCallIntent(intent)
    }

    override fun onDestroy() {
        if (CallChannel.activity === this) CallChannel.activity = null
        super.onDestroy()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleCallIntent(intent)
    }

    private fun handleCallIntent(intent: Intent?) {
        when (intent?.action) {
            IncomingCallNotification.ACTION_SHOW -> showOverLockScreen(true)
            MessageNotification.ACTION_OPEN_CHAT -> {
                val peer = intent.getStringExtra(MessageNotification.EXTRA_PEER) ?: return
                CallChannel.pendingChat = peer
                CallChannel.invoke("openChat", peer)
            }
            IncomingCallNotification.ACTION_ANSWER -> {
                showOverLockScreen(true)
                IncomingCallNotification.cancel(this)
                CallChannel.invoke("action", "accept")
            }
        }
    }

    /** Only while a call is ringing or active; otherwise the app stays behind the lock screen. */
    fun showOverLockScreen(on: Boolean) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            setShowWhenLocked(on)
            setTurnScreenOn(on)
        }
    }

    fun requestNotificationPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
    }
}
