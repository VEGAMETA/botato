package online.vegameta.botato

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    private var channel: MethodChannel? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val ch = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "botato/call")
        channel = ch
        CallForegroundService.onAction = { action -> runOnUiThread { ch.invokeMethod("action", action) } }
        ch.setMethodCallHandler { call, result ->
            when (call.method) {
                "show" -> {
                    requestNotificationPermission()
                    CallForegroundService.show(
                        this,
                        CallForegroundService.State(
                            peer = call.argument<String>("peer") ?: "",
                            status = call.argument<String>("status") ?: "",
                            muted = call.argument<Boolean>("muted") ?: false,
                            camera = call.argument<Boolean>("camera") ?: false,
                            screen = call.argument<Boolean>("screen") ?: false,
                        ),
                    )
                    result.success(null)
                }
                "types" -> {
                    CallForegroundService.setTypes(
                        this,
                        call.argument<Boolean>("camera") ?: false,
                        call.argument<Boolean>("screen") ?: false,
                    ) { runOnUiThread { result.success(null) } }
                }
                "hide" -> {
                    CallForegroundService.hide(this)
                    IncomingCallNotification.cancel(this)
                    showOverLockScreen(false)
                    result.success(null)
                }
                "incoming" -> {
                    requestNotificationPermission()
                    IncomingCallNotification.show(
                        this,
                        call.argument<String>("peer") ?: "",
                        call.argument<Boolean>("video") ?: false,
                    )
                    result.success(null)
                }
                "incomingCancel" -> {
                    IncomingCallNotification.cancel(this)
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        handleCallIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleCallIntent(intent)
    }

    private fun handleCallIntent(intent: Intent?) {
        when (intent?.action) {
            IncomingCallNotification.ACTION_SHOW -> showOverLockScreen(true)
            IncomingCallNotification.ACTION_ANSWER -> {
                showOverLockScreen(true)
                IncomingCallNotification.cancel(this)
                channel?.invokeMethod("action", "accept")
            }
        }
    }

    /** Only while a call is ringing or active; otherwise the app stays behind the lock screen. */
    private fun showOverLockScreen(on: Boolean) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            setShowWhenLocked(on)
            setTurnScreenOn(on)
        }
    }

    private fun requestNotificationPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
    }
}
