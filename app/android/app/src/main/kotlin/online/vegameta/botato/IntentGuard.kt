package online.vegameta.botato

import android.content.Intent
import java.security.SecureRandom

/**
 * MainActivity is exported (it is the launcher), so any app can start it with any action.
 * Actions with side effects (answer a call, show over the lock screen, open a chat) are
 * honoured only with this per-process secret, which only our own notifications carry.
 */
object IntentGuard {
    private const val EXTRA = "online.vegameta.botato.TOKEN"

    private val token: String = ByteArray(16).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it) }

    fun sign(intent: Intent): Intent = intent.putExtra(EXTRA, token)

    fun isOurs(intent: Intent?): Boolean = intent?.getStringExtra(EXTRA) == token
}
