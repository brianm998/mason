// SPDX-License-Identifier: GPL-3.0-or-later
// Make the macOS app icon from the full-bleed one: the artwork scaled into Apple's 824 x 824
// rounded rectangle (continuous corners, radius ~185) on a 1024 canvas, transparent outside it,
// with a soft drop shadow in the margin. macOS draws an icon exactly as given, so it must be
// shaped here. Run from the repository root after `app_icon.png` changes:
//   swift scripts/gen-app-icon-macos.swift
import AppKit
import CoreGraphics

let dir = "clients/swift/Sources/MasonMascot/Art/brand"
guard let src = NSImage(contentsOfFile: "\(dir)/app_icon.png"),
    let cg = src.cgImage(forProposedRect: nil, context: nil, hints: nil)
else {
    FileHandle.standardError.write(Data("cannot read app_icon.png\n".utf8))
    exit(1)
}

let canvas = 1024
let body: CGFloat = 824
let margin = (CGFloat(canvas) - body) / 2
let rect = CGRect(x: margin, y: margin, width: body, height: body)
let r: CGFloat = 185

/// A continuous-corner rounded rectangle (Apple's "squircle", approximated): the straight
/// edges stop 1.5287 * r from each corner and a pair of cubic curves per corner joins them.
func squircle(_ q: CGRect, _ r: CGFloat) -> CGPath {
    let k = 1.52866 * r
    let p = CGMutablePath()
    func pt(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x, y: y) }
    let (x0, y0, x1, y1) = (q.minX, q.minY, q.maxX, q.maxY)
    p.move(to: pt(x0 + k, y0))
    p.addLine(to: pt(x1 - k, y0))
    p.addCurve(to: pt(x1 - 0.63 * r, y0 + 0.07 * r), control1: pt(x1 - 0.55 * r, y0), control2: pt(x1 - 0.8 * r, y0 + 0.02 * r))
    p.addCurve(to: pt(x1, y0 + k), control1: pt(x1 - 0.2 * r, y0 + 0.2 * r), control2: pt(x1 - 0.02 * r, y0 + 0.55 * r))
    p.addLine(to: pt(x1, y1 - k))
    p.addCurve(to: pt(x1 - 0.63 * r, y1 - 0.07 * r), control1: pt(x1, y1 - 0.55 * r), control2: pt(x1 - 0.02 * r, y1 - 0.2 * r))
    p.addCurve(to: pt(x1 - k, y1), control1: pt(x1 - 0.8 * r, y1 - 0.02 * r), control2: pt(x1 - 0.55 * r, y1))
    p.addLine(to: pt(x0 + k, y1))
    p.addCurve(to: pt(x0 + 0.63 * r, y1 - 0.07 * r), control1: pt(x0 + 0.55 * r, y1), control2: pt(x0 + 0.8 * r, y1 - 0.02 * r))
    p.addCurve(to: pt(x0, y1 - k), control1: pt(x0 + 0.2 * r, y1 - 0.2 * r), control2: pt(x0 + 0.02 * r, y1 - 0.55 * r))
    p.addLine(to: pt(x0, y0 + k))
    p.addCurve(to: pt(x0 + 0.63 * r, y0 + 0.07 * r), control1: pt(x0, y0 + 0.55 * r), control2: pt(x0 + 0.02 * r, y0 + 0.2 * r))
    p.addCurve(to: pt(x0 + k, y0), control1: pt(x0 + 0.8 * r, y0 + 0.02 * r), control2: pt(x0 + 0.55 * r, y0))
    p.closeSubpath()
    return p
}

let space = CGColorSpace(name: CGColorSpace.sRGB)!
guard
    let ctx = CGContext(
        data: nil, width: canvas, height: canvas, bitsPerComponent: 8, bytesPerRow: 0,
        space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
else { exit(1) }
ctx.interpolationQuality = .high
let path = squircle(rect, r)

// Shadow, drawn with the body: offset down, inside the margin.
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: CGColor(gray: 0, alpha: 0.30))
ctx.addPath(path)
ctx.setFillColor(CGColor(gray: 0, alpha: 1))
ctx.fillPath()
ctx.restoreGState()

// The artwork, clipped to the shape and scaled to fill it.
ctx.saveGState()
ctx.addPath(path)
ctx.clip()
ctx.draw(cg, in: rect)
ctx.restoreGState()

guard let out = ctx.makeImage(),
    let png = NSBitmapImageRep(cgImage: out).representation(using: .png, properties: [:])
else { exit(1) }
try png.write(to: URL(fileURLWithPath: "\(dir)/app_icon_macos.png"))
print("wrote \(dir)/app_icon_macos.png")
