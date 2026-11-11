// 에셋 갱신용(일회성 개발 도구): GIF 애니메이션을 프레임을 가로로 이어 붙인 PNG 시트로 바꾼다.
//
// 사용: swift scripts/gif_to_sheet.swift <입력.gif> <출력.png> [칸 크기 비율]
//
// 칸 크기와 비율을 주면, 큰 해상도로 그려진 GIF를 앱의 스프라이트 칸에 맞게 줄여 넣는다.
// - 비율: 원본 px → 스프라이트 px (예: 0.2935)
// - 첫 프레임의 그림 영역을 기준으로 가로는 칸 가운데, 세로는 발이 바닥 줄(칸 아래에서 7px 위)에 닿게 놓는다.
// - 줄인 뒤 반투명 가장자리는 없앤다(다른 시트와 같은 또렷한 외곽선).
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}

func bitmap(_ w: Int, _ h: Int) -> CGContext {
    guard let c = CGContext(
        data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
        space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else { fail("cannot create bitmap") }
    return c
}

let args = CommandLine.arguments
guard args.count == 3 || args.count == 5,
      let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: args[1]) as CFURL, nil)
else { fail("usage: gif_to_sheet <in.gif> <out.png> [cell scale]") }
let count = CGImageSourceGetCount(source)
guard count > 0, let first = CGImageSourceCreateImageAtIndex(source, 0, nil) else { fail("no frames") }
let (w, h) = (first.width, first.height)
let cell = args.count == 5 ? Int(args[3])! : 0
let scale = args.count == 5 ? Double(args[4])! : 1.0

// 첫 프레임에서 그림이 있는 영역(위쪽이 0인 좌표).
var (minX, minY, maxX, maxY) = (w, h, -1, -1)
do {
    let c = bitmap(w, h)
    c.draw(first, in: CGRect(x: 0, y: 0, width: w, height: h))
    let p = c.data!.bindMemory(to: UInt8.self, capacity: w * h * 4)
    for y in 0..<h {
        for x in 0..<w where p[(y * w + x) * 4 + 3] > 127 {
            minX = min(minX, x); maxX = max(maxX, x); minY = min(minY, y); maxY = max(maxY, y)
        }
    }
}
guard maxX >= 0 else { fail("first frame is empty") }

let (outW, outH) = cell > 0 ? (cell, cell) : (w, h)
let sheet = bitmap(outW * count, outH)
sheet.interpolationQuality = cell > 0 ? .high : .none
for i in 0..<count {
    guard let frame = CGImageSourceCreateImageAtIndex(source, i, nil) else { fail("cannot read frame \(i)") }
    if cell > 0 {
        // 그림 영역의 가로 중심 → 칸 가운데, 아래 끝 → 바닥 줄(위에서 121번째 줄 바로 위).
        let centerX = Double(minX + maxX + 1) / 2
        let bottom = Double(maxY + 1)
        let originX = Double(i * cell) + Double(cell) / 2 - centerX * scale
        // CoreGraphics는 아래가 0이라, 위에서 잰 좌표를 뒤집어 준다.
        let groundFromTop = Double(cell) - 7
        let originYFromTop = groundFromTop - bottom * scale
        let rect = CGRect(x: originX, y: Double(cell) - originYFromTop - Double(h) * scale, width: Double(w) * scale, height: Double(h) * scale)
        sheet.saveGState()
        sheet.clip(to: CGRect(x: i * cell, y: 0, width: cell, height: cell))
        sheet.draw(frame, in: rect)
        sheet.restoreGState()
    } else {
        sheet.draw(frame, in: CGRect(x: i * w, y: 0, width: w, height: h))
    }
}
if cell > 0 {
    // 반투명 가장자리를 없애고 색을 프리멀티플라이 이전 값으로 되돌린다.
    let p = sheet.data!.bindMemory(to: UInt8.self, capacity: outW * count * outH * 4)
    for i in 0..<(outW * count * outH) {
        let a = Double(p[i * 4 + 3])
        if a >= 128 {
            for ch in 0..<3 { p[i * 4 + ch] = UInt8(min(255, (Double(p[i * 4 + ch]) * 255 / a).rounded())) }
            p[i * 4 + 3] = 255
        } else {
            for ch in 0..<4 { p[i * 4 + ch] = 0 }
        }
    }
}
guard let image = sheet.makeImage(),
      let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: args[2]) as CFURL, UTType.png.identifier as CFString, 1, nil)
else { fail("cannot write") }
CGImageDestinationAddImage(dest, image, nil)
guard CGImageDestinationFinalize(dest) else { fail("cannot write") }
print("\(count) frames, source \(w)x\(h), first-frame art \(maxX - minX + 1)x\(maxY - minY + 1), output \(outW)x\(outH) per frame")
