import Cocoa
import ApplicationServices

func now() -> Double { Date().timeIntervalSince1970 }
func attr(_ e: AXUIElement, _ a: String) -> AnyObject? { var v: CFTypeRef?; return AXUIElementCopyAttributeValue(e, a as CFString, &v) == .success ? (v as AnyObject?) : nil }
func kids(_ e: AXUIElement) -> [AXUIElement] { (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] }
func str(_ e: AXUIElement, _ a: String) -> String { (attr(e, a) as? String) ?? "" }
func frame(_ e: AXUIElement) -> CGRect? {
    guard let p = attr(e, kAXPositionAttribute), let s = attr(e, kAXSizeAttribute) else { return nil }
    var pt = CGPoint.zero, sz = CGSize.zero
    AXValueGetValue(p as! AXValue, .cgPoint, &pt); AXValueGetValue(s as! AXValue, .cgSize, &sz)
    return CGRect(origin: pt, size: sz)
}
func find(_ e: AXUIElement, depth: Int = 0, max: Int = 14, _ pred: (AXUIElement) -> Bool) -> AXUIElement? {
    if pred(e) { return e }
    if depth >= max { return nil }
    for c in kids(e) { if let r = find(c, depth: depth + 1, max: max, pred) { return r } }
    return nil
}
func findAll(_ e: AXUIElement, depth: Int = 0, max: Int = 14, _ pred: (AXUIElement) -> Bool, into: inout [AXUIElement]) {
    if pred(e) { into.append(e) }
    if depth >= max { return }
    for c in kids(e) { findAll(c, depth: depth + 1, max: max, pred, into: &into) }
}
func click(_ p: CGPoint) {
    for t in [CGEventType.leftMouseDown, .leftMouseUp] {
        let ev = CGEvent(mouseEventSource: nil, mouseType: t, mouseCursorPosition: p, mouseButton: .left)
        ev?.post(tap: .cghidEventTap)
    }
}
func key(_ code: CGKeyCode) { for d in [true, false] { CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: d)?.post(tap: .cghidEventTap) } }
func typeText(_ s: String) {
    for ch in s.utf16 {
        var c = ch
        for d in [true, false] { let ev = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: d); ev?.keyboardSetUnicodeString(stringLength: 1, unicodeString: &c); ev?.post(tap: .cghidEventTap) }
    }
}
func layers() -> Bool {
    guard let u = URL(string: "http://127.0.0.1:1025/v1/status/layers"), let d = try? Data(contentsOf: u),
          let j = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else { return false }
    return (j["slide"] as? Bool) ?? false
}

let args = CommandLine.arguments
let ref = args.count > 1 ? args[1] : "John 3:16"
let route = args.count > 2 ? args[2] : "click"          // click = click result in Bible view
guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: "com.renewedvision.propresenter").first else { print("ProPresenter not running"); exit(1) }
let ax = AXUIElementCreateApplication(app.processIdentifier)
let t0 = now()
func lap(_ label: String) { print(String(format: "%6.0f ms  %@", (now() - t0) * 1000, label)) }

guard let win = (attr(ax, kAXWindowsAttribute) as? [AXUIElement])?.first else { print("no window"); exit(1) }
lap("window found")
// scripture lookup field: the first plain text field inside the Bible panel (not a search field)
var fields: [AXUIElement] = []
findAll(win, { str($0, kAXRoleAttribute) == "AXTextField" && str($0, kAXSubroleAttribute) != "AXSearchField" && (frame($0).map { $0.width > 240 && $0.minX > 250 && $0.minX < 300 && $0.minY > 90 && $0.minY < 120 } ?? false) }, into: &fields)
fields.sort { frame($0)!.minY < frame($1)!.minY }
guard let lookup = fields.first else { print("lookup field not found (\(fields.count) candidates)"); exit(2) }
lap("lookup field found")
AXUIElementSetAttributeValue(lookup, kAXFocusedAttribute as CFString, kCFBooleanTrue)
AXUIElementSetAttributeValue(lookup, kAXValueAttribute as CFString, "" as CFString)
typeText(ref); lap("typed")
key(36); lap("return pressed")
// The result thumbnails are drawn on a canvas (not exposed), but the first one always sits at the top-left of the results area.
var area: CGRect? = nil
var scrolls: [AXUIElement] = []
findAll(win, { str($0, kAXRoleAttribute) == "AXScrollArea" && (frame($0).map { $0.width > 1200 && $0.height > 800 } ?? false) }, into: &scrolls)
area = scrolls.compactMap { frame($0) }.first
guard let r = area else { print("results area not found"); exit(3) }
let target = CGPoint(x: r.minX + 333, y: r.minY + 190)
let settle = args.count > 3 ? Double(args[3])! : 120          // ms to let the result render before clicking
usleep(UInt32(settle * 1000)); lap("waited \(Int(settle)) ms for result to render")
var tries = 0
while tries < 12 && !layers() {
    click(target); tries += 1; lap("click #\(tries) at \(Int(target.x)),\(Int(target.y))")
    let d = now() + 0.35
    while now() < d && !layers() { usleep(10_000) }
}
if layers() { lap("API says the slide layer is LIVE") } else { print("never went live") }
