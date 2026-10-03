import CoreGraphics
import Foundation
// Only identify the desktop process launched by this acceptance driver.
let target = Int(CommandLine.arguments[1])!
let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
for window in windows where (window[kCGWindowOwnerPID as String] as? Int) == target && (window[kCGWindowLayer as String] as? Int) == 0 {
    if let id = window[kCGWindowNumber as String] as? Int { print(id); break }
}
