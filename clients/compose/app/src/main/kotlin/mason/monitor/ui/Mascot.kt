// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.res.loadImageBitmap
import mason.MascotManifest

/**
 * The one way the Compose monitor loads the mascot art (the same files the SwiftUI monitor
 * bundles). Ids are the daemon's own strings; an id or size that is not in the manifest gives
 * null, never an exception. Every image is decorative: show it with `contentDescription = null`.
 */
object Mascot {
    private fun bytes(path: String): ByteArray? {
        if (path !in MascotManifest.files) return null
        return Mascot::class.java.classLoader?.getResourceAsStream(path)?.use { it.readBytes() }
    }

    private fun bitmap(path: String): ImageBitmap? =
        try {
            bytes(path)?.inputStream()?.use { loadImageBitmap(it) }
        } catch (e: Exception) {
            null
        }

    /** The resource path of a state icon at 20 or 24 (scale 1..3), or null when not in the manifest. */
    fun iconPath(id: String, size: Int, scale: Int = 2): String? =
        "icons/$id-$size@${scale}x.png".takeIf { id in MascotManifest.stateIds && it in MascotManifest.files }

    fun bannerIconPath(id: String, scale: Int = 2): String? =
        "banners/$id-20@${scale}x.png".takeIf { id in MascotManifest.bannerIds && it in MascotManifest.files }

    fun illustrationPath(id: String): String? =
        "illustrations/$id.png".takeIf { id in MascotManifest.illustrationIds && it in MascotManifest.files }

    /** A task state's icon at [size] (20 or 24) for a screen of [scale] (1, 2 or 3) pixels per dp. */
    fun icon(id: String, size: Int, scale: Int = 2): ImageBitmap? =
        iconPath(id, size, scale)?.let { bitmap(it) }

    fun bannerIcon(id: String, scale: Int = 2): ImageBitmap? =
        bannerIconPath(id, scale)?.let { bitmap(it) }

    fun illustration(id: String): ImageBitmap? = illustrationPath(id)?.let { bitmap(it) }
}
