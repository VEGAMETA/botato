package online.vegameta.botato

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import androidx.core.content.FileProvider
import java.io.File

/** "Share" in both directions: content other apps send to botato, and photos botato sends out. */
object Share {
    private const val MAX_FILES = 10
    private const val MAX_BYTES = 64L * 1024 * 1024

    fun isShare(intent: Intent?): Boolean =
        intent?.action == Intent.ACTION_SEND || intent?.action == Intent.ACTION_SEND_MULTIPLE

    /**
     * Reads a share intent into `{text, files}`: images are copied into <cache>/incoming/ (the
     * sender's URI permission ends with the intent). Runs on a background thread.
     */
    fun read(ctx: Context, intent: Intent): Map<String, Any?> {
        val dir = File(ctx.cacheDir, "incoming").apply {
            deleteRecursively()
            mkdirs()
        }
        val uris = mutableListOf<Uri>()
        if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
            intent.parcelableList(Intent.EXTRA_STREAM)?.let { uris.addAll(it) }
        } else {
            intent.parcelable(Intent.EXTRA_STREAM)?.let { uris.add(it) }
        }
        val files = mutableListOf<String>()
        // Only other apps' content: a file:// or our own provider's URI would let a malicious
        // sender make us read our private files (history database, keys) with our permissions.
        val foreign = uris.filter { it.scheme == "content" && it.authority?.startsWith(ctx.packageName) != true }
        for ((i, uri) in foreign.take(MAX_FILES).withIndex()) {
            val out = File(dir, "shared-$i")
            runCatching {
                ctx.contentResolver.openInputStream(uri)?.use { input ->
                    out.outputStream().use { o ->
                        val buf = ByteArray(64 * 1024)
                        var total = 0L
                        while (true) {
                            val n = input.read(buf)
                            if (n < 0) break
                            total += n
                            if (total > MAX_BYTES) error("too big")
                            o.write(buf, 0, n)
                        }
                    }
                }
                if (out.length() > 0) files.add(out.path)
            }.onFailure { out.delete() }
        }
        return mapOf("text" to intent.getStringExtra(Intent.EXTRA_TEXT), "files" to files)
    }

    /** Opens the system share sheet for a file under <cache>/share/. */
    fun sendFile(ctx: Context, path: String, mime: String) {
        val uri = FileProvider.getUriForFile(ctx, "${ctx.packageName}.updates", File(path))
        val send = Intent(Intent.ACTION_SEND)
            .setType(mime)
            .putExtra(Intent.EXTRA_STREAM, uri)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        ctx.startActivity(Intent.createChooser(send, null).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    @Suppress("DEPRECATION")
    private fun Intent.parcelable(key: String): Uri? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) getParcelableExtra(key, Uri::class.java)
        else getParcelableExtra(key)

    @Suppress("DEPRECATION")
    private fun Intent.parcelableList(key: String): List<Uri>? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) getParcelableArrayListExtra(key, Uri::class.java)
        else getParcelableArrayListExtra(key)
}
