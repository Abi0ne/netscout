// Draws the NetScout app icon: a radar sweeping a small network, devices
// lighting up as the beam finds them.
//
//   swift scripts/make-icon.swift <out.png>    # 1024×1024 master
//
// scripts/make-icon.sh turns the master into apple/Resources/AppIcon.icns.

import AppKit

let size: CGFloat = 1024
let out = CommandLine.arguments.dropFirst().first ?? "AppIcon.png"

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
            green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: alpha)
}

let teal: UInt32 = 0x34E8C8
let space = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8, bytesPerRow: 0,
                    space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

// macOS icon grid: an 824-pt rounded square centred on the 1024 canvas.
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let tilePath = CGPath(roundedRect: tile, cornerWidth: 185, cornerHeight: 185, transform: nil)

// Drop shadow under the tile.
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: color(0x000000, 0.35))
ctx.addPath(tilePath)
ctx.setFillColor(color(0x08182B))
ctx.fillPath()
ctx.restoreGState()

ctx.saveGState()
ctx.addPath(tilePath)
ctx.clip()

// Background: deep navy, lighter towards the centre.
let bg = CGGradient(colorsSpace: space, colors: [color(0x123B5E), color(0x0A2238), color(0x050F1C)] as CFArray,
                    locations: [0, 0.55, 1])!
let c = CGPoint(x: 512, y: 512)
ctx.drawRadialGradient(bg, startCenter: CGPoint(x: 512, y: 560), startRadius: 0,
                       endCenter: c, endRadius: 620, options: [.drawsAfterEndLocation])

// Rings and crosshair.
ctx.setLineWidth(5)
for r in [120.0, 230.0, 340.0] as [CGFloat] {
    ctx.setStrokeColor(color(teal, 0.22))
    ctx.strokeEllipse(in: CGRect(x: c.x - r, y: c.y - r, width: 2 * r, height: 2 * r))
}
ctx.setStrokeColor(color(teal, 0.12))
ctx.setLineWidth(4)
ctx.strokeLineSegments(between: [CGPoint(x: c.x - 340, y: c.y), CGPoint(x: c.x + 340, y: c.y),
                                 CGPoint(x: c.x, y: c.y - 340), CGPoint(x: c.x, y: c.y + 340)])

// Sweep: a wedge fading behind the leading edge (angles in degrees, CCW).
let lead: CGFloat = 58
let trail: CGFloat = 70
let radius: CGFloat = 340
let steps = 90
for i in 0..<steps {
    let a0 = (lead - trail * CGFloat(i + 1) / CGFloat(steps)) * .pi / 180
    let a1 = (lead - trail * CGFloat(i) / CGFloat(steps)) * .pi / 180
    let alpha = 0.42 * pow(1 - CGFloat(i) / CGFloat(steps), 1.6)
    ctx.move(to: c)
    ctx.addArc(center: c, radius: radius, startAngle: a0, endAngle: a1 + 0.002, clockwise: false)
    ctx.closePath()
    ctx.setFillColor(color(teal, alpha))
    ctx.fillPath()
}
let edge = lead * .pi / 180
ctx.setStrokeColor(color(teal, 0.95))
ctx.setLineWidth(7)
ctx.setLineCap(.round)
ctx.strokeLineSegments(between: [c, CGPoint(x: c.x + radius * cos(edge), y: c.y + radius * sin(edge))])

// The network: devices on the rings, linked to the scanner and to each other.
struct Node { let angle: CGFloat; let r: CGFloat; let found: Bool }
let nodes = [
    Node(angle: 35, r: 230, found: true),
    Node(angle: 12, r: 330, found: true),
    Node(angle: 140, r: 250, found: false),
    Node(angle: 205, r: 160, found: false),
    Node(angle: 250, r: 300, found: false),
    Node(angle: 320, r: 210, found: false),
]
func point(_ n: Node) -> CGPoint {
    let a = n.angle * .pi / 180
    return CGPoint(x: c.x + n.r * cos(a), y: c.y + n.r * sin(a))
}
ctx.setLineWidth(5)
ctx.setStrokeColor(color(0xFFFFFF, 0.22))
for n in nodes { ctx.strokeLineSegments(between: [c, point(n)]) }
ctx.strokeLineSegments(between: [point(nodes[0]), point(nodes[1]), point(nodes[2]), point(nodes[3]),
                                 point(nodes[4]), point(nodes[5])])

for n in nodes {
    let p = point(n)
    let r: CGFloat = n.found ? 30 : 22
    ctx.saveGState()
    if n.found {
        ctx.setShadow(offset: .zero, blur: 40, color: color(teal, 1))
        ctx.setFillColor(color(teal))
    } else {
        ctx.setFillColor(color(0x8FB3D1, 0.85))
    }
    ctx.fillEllipse(in: CGRect(x: p.x - r, y: p.y - r, width: 2 * r, height: 2 * r))
    ctx.restoreGState()
    if n.found {
        ctx.setStrokeColor(color(teal, 0.45))
        ctx.setLineWidth(5)
        ctx.strokeEllipse(in: CGRect(x: p.x - r - 16, y: p.y - r - 16, width: 2 * (r + 16), height: 2 * (r + 16)))
    }
}

// The scanner at the centre.
ctx.saveGState()
ctx.setShadow(offset: .zero, blur: 36, color: color(0xFFFFFF, 0.8))
ctx.setFillColor(color(0xFFFFFF))
ctx.fillEllipse(in: CGRect(x: c.x - 34, y: c.y - 34, width: 68, height: 68))
ctx.restoreGState()
ctx.setFillColor(color(0x0A2238))
ctx.fillEllipse(in: CGRect(x: c.x - 14, y: c.y - 14, width: 28, height: 28))

// A faint top highlight gives the tile some depth.
let gloss = CGGradient(colorsSpace: space, colors: [color(0xFFFFFF, 0.10), color(0xFFFFFF, 0)] as CFArray,
                       locations: [0, 1])!
ctx.drawLinearGradient(gloss, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 600), options: [])
ctx.restoreGState()

let image = ctx.makeImage()!
let png = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])!
try png.write(to: URL(fileURLWithPath: out))
print("wrote \(out)")
