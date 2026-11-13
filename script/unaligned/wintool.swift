// Window helper for script/unaligned/shot (macOS). Compiled on first use into .work/bin/wintool.
//
//   wintool list <pid>    JSON array of the windows of a process (normal and floating), front to back:
//                         [{"id":…, "x":…, "y":…, "width":…, "height":…, "onscreen":…, "title":…}]
//   wintool check <png>   prints "<width> <height> <distinct colours> <share of the main colour>",
//                         exit status 1 when the image is blank (one flat colour)
//   wintool locked        prints "locked" or "unlocked" (login session screen lock)
import CoreGraphics
import Foundation
import ImageIO

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(2)
}

func windows(of pid: Int32) -> [[String: Any]] {
    var seen = Set<Int>()
    var result: [[String: Any]] = []
    // The on-screen list is ordered front to back; the full list adds windows on other Spaces.
    for (options, onscreen) in [(CGWindowListOption.optionOnScreenOnly, true), (CGWindowListOption.optionAll, false)] {
        let list = CGWindowListCopyWindowInfo([options], kCGNullWindowID) as? [[String: Any]] ?? []
        for window in list {
            guard let owner = window[kCGWindowOwnerPID as String] as? Int32, owner == pid,
                  let layer = window[kCGWindowLayer as String] as? Int, layer >= 0, layer < 20,
                  let id = window[kCGWindowNumber as String] as? Int, !seen.contains(id),
                  let bounds = window[kCGWindowBounds as String] as? [String: Double]
            else { continue }
            let width = bounds["Width"] ?? 0, height = bounds["Height"] ?? 0
            if width * height < 10000 { continue }
            seen.insert(id)
            result.append([
                "id": id, "x": bounds["X"] ?? 0, "y": bounds["Y"] ?? 0, "width": width, "height": height,
                "onscreen": onscreen, "title": window[kCGWindowName as String] as? String ?? "",
            ])
        }
    }
    return result
}

func check(_ path: String) -> Never {
    guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
    else { fail("cannot read image \(path)") }
    // Sample on a reduced grid without interpolation, so flat areas keep their exact colour.
    let scale = max(1, image.width / 480)
    let width = max(1, image.width / scale), height = max(1, image.height / scale)
    var pixels = [UInt8](repeating: 0, count: width * height * 4)
    guard let context = CGContext(
        data: &pixels, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
        space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
    else { fail("cannot create a bitmap context") }
    context.interpolationQuality = .none
    context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    var counts: [UInt32: Int] = [:]
    for index in stride(from: 0, to: pixels.count, by: 4) {
        let key = UInt32(pixels[index] >> 2) << 16 | UInt32(pixels[index + 1] >> 2) << 8 | UInt32(pixels[index + 2] >> 2)
        counts[key, default: 0] += 1
    }
    let share = Double(counts.values.max() ?? 0) / Double(width * height)
    print("\(image.width) \(image.height) \(counts.count) \(String(format: "%.4f", share))")
    exit(counts.count < 6 || share > 0.995 ? 1 : 0)
}

let arguments = CommandLine.arguments
guard arguments.count >= 2 else { fail("usage: wintool list <pid> | check <png> | locked") }
switch arguments[1] {
case "list":
    guard arguments.count == 3, let pid = Int32(arguments[2]) else { fail("usage: wintool list <pid>") }
    let data = try! JSONSerialization.data(withJSONObject: windows(of: pid))
    print(String(data: data, encoding: .utf8)!)
case "check":
    guard arguments.count == 3 else { fail("usage: wintool check <png>") }
    check(arguments[2])
case "locked":
    let session = CGSessionCopyCurrentDictionary() as? [String: Any] ?? [:]
    let locked = (session["CGSSessionScreenIsLocked"] as? Int ?? 0) != 0
    print(locked ? "locked" : "unlocked")
default:
    fail("unknown command \(arguments[1])")
}
