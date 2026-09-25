package online.vegameta.botato

import android.content.Context
import android.media.AudioAttributes
import android.media.MediaPlayer
import android.net.Uri

/**
 * In-call tones (ringback, reconnecting, hangup, screen sharing) from res/raw
 * (tools/gen-sounds.py). They play as voice communication, so they follow the call's
 * route (earpiece, loudspeaker, headset) and take no audio focus from the call.
 */
object Sounds {
    private var loop: MediaPlayer? = null
    private var loopName: String? = null

    fun uri(ctx: Context, name: String): Uri =
        Uri.parse("android.resource://${ctx.packageName}/raw/$name")

    fun play(ctx: Context, name: String, looping: Boolean) {
        if (looping) {
            if (loopName == name && loop != null) return
            stopLoop()
        }
        val player = runCatching {
            MediaPlayer().apply {
                setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                        .build(),
                )
                setDataSource(ctx.applicationContext, uri(ctx, name))
                isLooping = looping
                prepare()
            }
        }.getOrNull() ?: return
        if (looping) {
            loop = player
            loopName = name
        } else {
            player.setOnCompletionListener { it.release() }
        }
        player.start()
    }

    fun stopLoop() {
        loop?.let { runCatching { it.stop() }; it.release() }
        loop = null
        loopName = null
    }
}
