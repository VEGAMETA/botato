package online.vegameta.botato

import android.content.Context
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Build

/** One line for the call log: what Android's audio is doing right now (no addresses, no content). */
object AudioDiag {
    fun describe(ctx: Context): String {
        val am = ctx.getSystemService(Context.AUDIO_SERVICE) as AudioManager
        val mode = when (am.mode) {
            AudioManager.MODE_NORMAL -> "normal"
            AudioManager.MODE_IN_COMMUNICATION -> "communication"
            AudioManager.MODE_IN_CALL -> "in-call"
            AudioManager.MODE_RINGTONE -> "ringtone"
            else -> am.mode.toString()
        }
        val sb = StringBuilder("mode=$mode")
        if (Build.VERSION.SDK_INT >= 31) sb.append(" comm=").append(am.communicationDevice?.let { type(it.type) } ?: "none")
        @Suppress("DEPRECATION")
        sb.append(" speakerphone=").append(am.isSpeakerphoneOn)
        sb.append(" micMute=").append(am.isMicrophoneMute)
        fun vol(stream: Int) = "${am.getStreamVolume(stream)}/${am.getStreamMaxVolume(stream)}"
        sb.append(" vol[call=").append(vol(AudioManager.STREAM_VOICE_CALL))
            .append(" music=").append(vol(AudioManager.STREAM_MUSIC)).append(']')
        if (Build.VERSION.SDK_INT >= 29) {
            for (rc in am.activeRecordingConfigurations) {
                sb.append(" rec[src=").append(source(rc.clientAudioSource))
                    .append(" dev=").append(rc.audioDevice?.let { type(it.type) } ?: "?")
                    .append(" silenced=").append(rc.isClientSilenced)
                    .append(" fx=").append(rc.clientEffects.joinToString(",") { it.name }.ifEmpty { "-" })
                    .append(']')
            }
        }
        return sb.toString()
    }

    private fun source(s: Int) = when (s) {
        1 -> "mic"
        7 -> "voice-communication"
        6 -> "voice-recognition"
        9 -> "unprocessed"
        10 -> "voice-performance"
        else -> s.toString()
    }

    private fun type(t: Int) = when (t) {
        AudioDeviceInfo.TYPE_BUILTIN_EARPIECE -> "earpiece"
        AudioDeviceInfo.TYPE_BUILTIN_SPEAKER -> "speaker"
        AudioDeviceInfo.TYPE_BUILTIN_MIC -> "builtin-mic"
        AudioDeviceInfo.TYPE_WIRED_HEADSET -> "wired-headset"
        AudioDeviceInfo.TYPE_WIRED_HEADPHONES -> "wired-headphones"
        AudioDeviceInfo.TYPE_BLUETOOTH_SCO -> "bt-sco"
        AudioDeviceInfo.TYPE_BLUETOOTH_A2DP -> "bt-a2dp"
        AudioDeviceInfo.TYPE_USB_HEADSET -> "usb-headset"
        26 -> "ble-headset"
        else -> t.toString()
    }
}
