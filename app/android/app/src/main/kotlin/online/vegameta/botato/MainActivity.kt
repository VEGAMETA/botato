package online.vegameta.botato

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        val channel = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "botato/call")
        CallForegroundService.onAction = { action -> runOnUiThread { channel.invokeMethod("action", action) } }
        channel.setMethodCallHandler { call, result ->
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
                    result.success(null)
                }
                else -> result.notImplemented()
            }
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
