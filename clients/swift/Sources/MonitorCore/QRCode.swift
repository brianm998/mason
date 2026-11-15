// SPDX-License-Identifier: GPL-3.0-or-later

#if canImport(CoreImage)
import CoreImage
import CoreImage.CIFilterBuiltins
import Foundation

/// A QR code for a pairing URL (s433), drawn by CoreImage's `CIQRCodeGenerator`. Offscreen: it
/// makes a bitmap and never opens a window.
public enum QRCode {
    /// A black-on-white image `side` pixels square (scaled with no smoothing so modules stay
    /// sharp), with the quiet zone the generator adds. nil when the text cannot be encoded.
    public static func image(for text: String, side: Int = 480) -> CGImage? {
        let f = CIFilter.qrCodeGenerator()
        f.message = Data(text.utf8)
        f.correctionLevel = "M"
        guard let out = f.outputImage, out.extent.width > 0 else { return nil }
        let scale = max(1, CGFloat(side) / out.extent.width)
        let big = out.transformed(by: CGAffineTransform(scaleX: scale, y: scale), highQualityDownsample: false)
            .samplingNearest()
        return CIContext(options: [.useSoftwareRenderer: true]).createCGImage(big, from: big.extent)
    }

    /// What a scanner reads back from `image`: the encoded text, or nil.
    public static func decode(_ image: CGImage) -> String? {
        let det = CIDetector(ofType: CIDetectorTypeQRCode, context: nil, options: [CIDetectorAccuracy: CIDetectorAccuracyHigh])
        let feats = det?.features(in: CIImage(cgImage: image)) ?? []
        return (feats.first as? CIQRCodeFeature)?.messageString
    }
}
#endif
