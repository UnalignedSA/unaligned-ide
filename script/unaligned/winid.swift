import CoreGraphics
import Foundation
// Prints the id of the largest normal window of the given pid (all Spaces).
let pid = Int32(CommandLine.arguments[1])!
let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String: Any]]
var best: (Int, Double)? = nil
for w in list {
    guard let owner = w[kCGWindowOwnerPID as String] as? Int32, owner == pid,
          let layer = w[kCGWindowLayer as String] as? Int, layer == 0,
          let id = w[kCGWindowNumber as String] as? Int,
          let b = w[kCGWindowBounds as String] as? [String: Double] else { continue }
    let area = (b["Width"] ?? 0) * (b["Height"] ?? 0)
    if area > 10000, best == nil || area > best!.1 { best = (id, area) }
}
if let b = best { print(b.0) } else { exit(1) }
