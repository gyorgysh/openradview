// SPDX-License-Identifier: GPL-3.0-or-later
// Original, code-drawn installer artwork. No browser, window or screen capture.
import AppKit
import CoreText

guard CommandLine.arguments.count == 2 else {
    throw NSError(domain: "OpenRadView", code: 1, userInfo: [NSLocalizedDescriptionKey: "Pass the repository root"])
}
let root = URL(fileURLWithPath: CommandLine.arguments[1])
let destination = root.appendingPathComponent("packaging/macos/installer")
let fontURL = root.appendingPathComponent("assets/fonts/SpaceGrotesk.ttf")
CTFontManagerRegisterFontsForURL(fontURL as CFURL, .process, nil)
guard let wordmark = NSImage(contentsOf: root.appendingPathComponent("assets/brand/logo/horizontal/openradview-horizontal-grayscale-inverse-1024.png")) else {
    throw NSError(domain: "OpenRadView", code: 2, userInfo: [NSLocalizedDescriptionKey: "Missing supplied wordmark"])
}
let ink = NSColor(srgbRed: 10/255, green: 10/255, blue: 10/255, alpha: 1)
let silver = NSColor(white: 179/255, alpha: 1)
func text(_ value: String, x: CGFloat, y: CGFloat, size: CGFloat, color: NSColor) {
    let font = NSFont(name: "SpaceGrotesk-Regular", size: size) ?? NSFont.systemFont(ofSize: size)
    (value as NSString).draw(at: NSPoint(x: x, y: y), withAttributes: [.font: font, .foregroundColor: color])
}
for scale in [1, 2] {
    guard let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 660*scale, pixelsHigh: 420*scale,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
        let graphics = NSGraphicsContext(bitmapImageRep: bitmap) else {
        throw NSError(domain: "OpenRadView", code: 3, userInfo: [NSLocalizedDescriptionKey: "Cannot allocate installer artwork"])
    }
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = graphics
    let cg = graphics.cgContext
    cg.translateBy(x: 0, y: CGFloat(420*scale))
    cg.scaleBy(x: CGFloat(scale), y: -CGFloat(scale))
    NSGraphicsContext.current = NSGraphicsContext(cgContext: cg, flipped: true)
    ink.setFill()
    NSBezierPath(rect: NSRect(x: 0, y: 0, width: 660, height: 420)).fill()
    wordmark.draw(in: NSRect(x: 42, y: 34, width: 252, height: 46), from: .zero,
                  operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
    text("Your images. Your device.", x: 44, y: 92, size: 19, color: .white)
    text("Drag OpenRadView into Applications to install.", x: 44, y: 124, size: 13, color: silver)
    // Finder chooses its own filename colour for image backgrounds. Give its
    // black labels a stable light surface instead of relying on private prefs.
    NSColor(white: 242/255, alpha: 1).setFill()
    NSBezierPath(roundedRect: NSRect(x: 84, y: 160, width: 492, height: 154), xRadius: 18, yRadius: 18).fill()
    let arrow = NSBezierPath()
    arrow.move(to: NSPoint(x: 282, y: 222)); arrow.line(to: NSPoint(x: 376, y: 222))
    arrow.move(to: NSPoint(x: 365, y: 211)); arrow.line(to: NSPoint(x: 376, y: 222)); arrow.line(to: NSPoint(x: 365, y: 233))
    arrow.lineWidth = 1.5; NSColor(white: 107/255, alpha: 1).setStroke(); arrow.stroke()
    text("View, explore, learn from your own scans.", x: 44, y: 342, size: 14, color: .white)
    text("Personal research only. Not for diagnosis or clinical decisions.", x: 44, y: 372, size: 11, color: silver)
    NSGraphicsContext.restoreGraphicsState()
    guard let data = bitmap.representation(using: .png, properties: [:]) else {
        throw NSError(domain: "OpenRadView", code: 4, userInfo: [NSLocalizedDescriptionKey: "Cannot encode installer artwork"])
    }
    let name = scale == 1 ? "background.png" : "background@2x.png"
    try data.write(to: destination.appendingPathComponent(name), options: .atomic)
    print("Rendered \(name) (\(660*scale) × \(420*scale))")
}
