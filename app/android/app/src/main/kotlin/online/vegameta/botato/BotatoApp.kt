package online.vegameta.botato

import android.app.Application
import android.content.Context
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.embedding.engine.FlutterEngineCache
import io.flutter.embedding.engine.dart.DartExecutor

/**
 * One Flutter engine per process, shared by the activity and by push wake-ups: a push can
 * start the app headless (no activity) and the call/relay state it builds is the same the
 * UI attaches to when the user opens the app or answers from the notification.
 */
class BotatoApp : Application() {
    companion object {
        const val ENGINE_ID = "main"

        /** Must run on the main thread. */
        fun engine(ctx: Context): FlutterEngine {
            FlutterEngineCache.getInstance().get(ENGINE_ID)?.let { return it }
            val app = ctx.applicationContext
            val engine = FlutterEngine(app)
            CallChannel.attach(engine, app)
            engine.dartExecutor.executeDartEntrypoint(DartExecutor.DartEntrypoint.createDefault())
            FlutterEngineCache.getInstance().put(ENGINE_ID, engine)
            return engine
        }

        fun hasEngine(): Boolean = FlutterEngineCache.getInstance().contains(ENGINE_ID)
    }

    override fun onCreate() {
        super.onCreate()
        // Uncaught Kotlin/Java exceptions go to the app's error journal (<files>/errors.log,
        // shared with Dart and Rust), then to the system's handler as before.
        val journal = java.io.File(filesDir, "errors.log")
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, e ->
            runCatching {
                journal.appendText("${System.currentTimeMillis() / 1000} [kotlin] ${thread.name}: ${e.stackTraceToString().take(4000)}\n")
            }
            previous?.uncaughtException(thread, e)
        }
    }
}
