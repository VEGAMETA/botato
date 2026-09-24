package online.vegameta.botato

import android.content.Context
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Build
import android.os.PowerManager
import java.util.concurrent.Executor

/**
 * Screen off while the phone is at the ear: only during a call that plays through the
 * earpiece (not the loudspeaker, a headset or Bluetooth).
 */
object Proximity {
    private var wakeLock: PowerManager.WakeLock? = null
    private var wanted = false
    private var listener: Any? = null

    /** [want]: a call is up and the user did not pick the loudspeaker. Main thread. */
    fun set(ctx: Context, want: Boolean) {
        val app = ctx.applicationContext
        wanted = want
        val audio = app.getSystemService(AudioManager::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            if (want && listener == null) {
                val l = AudioManager.OnCommunicationDeviceChangedListener { apply(app) }
                audio.addOnCommunicationDeviceChangedListener(Executor { it.run() }, l)
                listener = l
            } else if (!want && listener != null) {
                audio.removeOnCommunicationDeviceChangedListener(
                    listener as AudioManager.OnCommunicationDeviceChangedListener,
                )
                listener = null
            }
        }
        apply(app)
    }

    private fun apply(ctx: Context) {
        val on = wanted && earpiece(ctx)
        val lock = wakeLock
        if (on && lock == null) {
            val power = ctx.getSystemService(PowerManager::class.java)
            if (!power.isWakeLockLevelSupported(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK)) return
            wakeLock = power.newWakeLock(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK, "botato:proximity")
                .apply {
                    setReferenceCounted(false)
                    acquire()
                }
        } else if (!on && lock != null) {
            // Wait for the phone to leave the ear before turning the screen back on.
            if (lock.isHeld) lock.release(PowerManager.RELEASE_FLAG_WAIT_FOR_NO_PROXIMITY)
            wakeLock = null
        }
    }

    private fun earpiece(ctx: Context): Boolean {
        val audio = ctx.getSystemService(AudioManager::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // Null before WebRTC picks a route: treat as earpiece, the default for calls.
            val type = audio.communicationDevice?.type ?: return true
            return type == AudioDeviceInfo.TYPE_BUILTIN_EARPIECE
        }
        @Suppress("DEPRECATION")
        return !audio.isSpeakerphoneOn && !audio.isBluetoothScoOn && !audio.isWiredHeadsetOn
    }
}
