// SPDX-License-Identifier: GPL-3.0-or-later
package mason.android.ui

import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.NotFoundException
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import java.util.concurrent.Executors

/**
 * The camera, reading QR codes only (ZXing on CameraX's luminance frames). Not unit tested: it
 * needs a camera. The text it reads goes through `PairingScreenState.scanned`.
 */
@Composable
fun QrScanner(onText: (String) -> Unit, modifier: Modifier = Modifier) {
    val ctx = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val latest = rememberUpdatedState(onText)
    val pool = remember { Executors.newSingleThreadExecutor() }
    val view = remember { PreviewView(ctx) }
    DisposableEffect(owner) {
        val future = ProcessCameraProvider.getInstance(ctx)
        val listener = Runnable {
            val provider = future.get()
            val preview = Preview.Builder().build().also { it.surfaceProvider = view.surfaceProvider }
            val analysis = ImageAnalysis.Builder()
                .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST).build()
            analysis.setAnalyzer(pool) { image ->
                decode(image)?.let { text -> ContextCompat.getMainExecutor(ctx).execute { latest.value(text) } }
            }
            provider.unbindAll()
            provider.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
        }
        future.addListener(listener, ContextCompat.getMainExecutor(ctx))
        onDispose { runCatching { future.get().unbindAll() }; pool.shutdown() }
    }
    AndroidView(factory = { view }, modifier = modifier)
}

private val reader = QRCodeReader()
private val hints = mapOf(DecodeHintType.TRY_HARDER to true)

private fun decode(image: ImageProxy): String? = image.use {
    val plane = it.planes[0]
    val buf = plane.buffer
    val bytes = ByteArray(buf.remaining()).also { b -> buf.get(b) }
    val source = PlanarYUVLuminanceSource(bytes, plane.rowStride, it.height, 0, 0, it.width, it.height, false)
    try {
        reader.decode(BinaryBitmap(HybridBinarizer(source)), hints).text
    } catch (_: NotFoundException) {
        null
    } catch (_: com.google.zxing.ChecksumException) {
        null
    } catch (_: com.google.zxing.FormatException) {
        null
    } finally {
        reader.reset()
    }
}
