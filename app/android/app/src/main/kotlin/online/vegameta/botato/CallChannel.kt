package online.vegameta.botato

import android.content.Context
import android.os.Handler
import android.os.Looper
import com.google.firebase.messaging.FirebaseMessaging
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

/**
 * The "botato/call" channel, bound to the engine rather than to an activity, so it works
 * when the app runs headless after a push. Activity-only bits (permission prompts, showing
 * over the lock screen) are skipped while there is no activity.
 */
object CallChannel {
    private var channel: MethodChannel? = null
    private val main = Handler(Looper.getMainLooper())

    /** The visible activity, if any. */
    var activity: MainActivity? = null

    /** Chat to open when Dart asks (cold start from a notification tap). */
    var pendingChat: String? = null

    /** FLAG_SECURE wanted (Dart's setting); on until Dart says otherwise. */
    var secure = true

    /** Content shared from another app, until Dart takes it: `{text, files}`. */
    @Volatile var pendingShare: Map<String, Any?>? = null

    fun invoke(method: String, arg: Any?) {
        main.post { channel?.invokeMethod(method, arg) }
    }

    fun attach(engine: FlutterEngine, app: Context) {
        val ch = MethodChannel(engine.dartExecutor.binaryMessenger, "botato/call")
        channel = ch
        CallForegroundService.onAction = { action -> invoke("action", action) }
        ch.setMethodCallHandler { call, result ->
            val ctx: Context = activity ?: app
            when (call.method) {
                "show" -> {
                    activity?.requestNotificationPermission()
                    CallForegroundService.show(
                        ctx,
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
                "types" -> CallForegroundService.setTypes(
                    ctx,
                    call.argument<Boolean>("camera") ?: false,
                    call.argument<Boolean>("screen") ?: false,
                ) { main.post { result.success(null) } }
                "hide" -> {
                    Proximity.set(ctx, false)
                    CallForegroundService.hide(ctx)
                    IncomingCallNotification.cancel(ctx)
                    activity?.showOverLockScreen(false)
                    result.success(null)
                }
                "incoming" -> {
                    activity?.requestNotificationPermission()
                    // A stopped activity brought back by the full-screen intent stays under the
                    // keyguard (its show-when-locked is decided too late). Finish it: the intent
                    // then creates a fresh one, which sets the flags in onCreate. The Flutter
                    // engine is cached, so the call and the relay session go on. A visible
                    // activity is kept and just allowed over the lock screen.
                    activity?.let { a ->
                        if (a.lifecycle.currentState.isAtLeast(androidx.lifecycle.Lifecycle.State.RESUMED)) {
                            a.showOverLockScreen(true)
                        } else {
                            a.finish()
                        }
                    }
                    IncomingCallNotification.show(
                        ctx,
                        call.argument<String>("peer") ?: "",
                        call.argument<Boolean>("video") ?: false,
                    )
                    result.success(null)
                }
                "incomingCancel" -> {
                    IncomingCallNotification.cancel(ctx)
                    result.success(null)
                }
                "message" -> {
                    activity?.requestNotificationPermission()
                    MessageNotification.show(
                        ctx,
                        call.argument<String>("peer") ?: "",
                        call.argument<String>("name") ?: "",
                        call.argument<String>("text") ?: "",
                    )
                    result.success(null)
                }
                "messageClear" -> {
                    MessageNotification.cancel(ctx, call.argument<String>("peer") ?: "")
                    result.success(null)
                }
                "proximity" -> {
                    Proximity.set(ctx, call.arguments as Boolean)
                    result.success(null)
                }
                "installApk" -> {
                    val path = call.argument<String>("path") ?: ""
                    if (Updates.install(ctx, path)) {
                        result.success(null)
                    } else {
                        result.error("permission", "разрешите botato устанавливать приложения и нажмите «Обновить» ещё раз", null)
                    }
                }
                "saveImage" -> {
                    val name = call.argument<String>("name") ?: "botato.jpg"
                    val data = call.argument<ByteArray>("data") ?: ByteArray(0)
                    val mime = call.argument<String>("mime") ?: "image/jpeg"
                    Thread {
                        val err = runCatching { Gallery.save(app, name, mime, data) }.exceptionOrNull()
                        main.post {
                            if (err == null) result.success(null) else result.error("save", err.message, null)
                        }
                    }.start()
                }
                "secureScreen" -> {
                    secure = call.arguments as Boolean
                    activity?.applySecure()
                    result.success(null)
                }
                // Android 14+: full-screen notifications need a special permission, off by
                // default for apps not installed from Google Play. Without it an incoming call on
                // a locked phone is only a notification.
                "fullScreenAllowed" -> {
                    val allowed = android.os.Build.VERSION.SDK_INT < 34 ||
                        ctx.getSystemService(android.app.NotificationManager::class.java).canUseFullScreenIntent()
                    result.success(allowed)
                }
                "openFullScreenSettings" -> {
                    runCatching {
                        ctx.startActivity(
                            android.content.Intent(
                                android.provider.Settings.ACTION_MANAGE_APP_USE_FULL_SCREEN_INTENT,
                                android.net.Uri.parse("package:${ctx.packageName}"),
                            ).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK),
                        )
                    }
                    result.success(null)
                }
                "pendingShare" -> {
                    result.success(pendingShare)
                    pendingShare = null
                }
                "shareFile" -> {
                    val path = call.argument<String>("path") ?: ""
                    val mime = call.argument<String>("mime") ?: "image/jpeg"
                    runCatching { Share.sendFile(ctx, path, mime) }
                        .onSuccess { result.success(null) }
                        .onFailure { result.error("share", it.message, null) }
                }
                "pendingChat" -> {
                    result.success(pendingChat)
                    pendingChat = null
                }
                "pushToken" -> FirebaseMessaging.getInstance().token.addOnCompleteListener { t ->
                    main.post { result.success(if (t.isSuccessful) t.result else null) }
                }
                else -> result.notImplemented()
            }
        }
    }
}
