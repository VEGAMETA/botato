package online.vegameta.botato

import android.content.Context
import java.util.Locale

/**
 * UI language for notifications. Dart sets it (profile → Language); it is saved because a
 * push may start the process before Dart runs. Until then: the system language.
 */
object Lang {
    private const val PREFS = "botato_lang"
    @Volatile private var english: Boolean? = null

    fun set(ctx: Context, en: Boolean) {
        english = en
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean("english", en).apply()
    }

    fun english(ctx: Context): Boolean = english ?: run {
        val prefs = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val en = if (prefs.contains("english")) prefs.getBoolean("english", false) else Locale.getDefault().language != "ru"
        english = en
        en
    }

    fun tr(ctx: Context, ru: String, en: String) = if (english(ctx)) en else ru
}
