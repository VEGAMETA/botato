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
}
