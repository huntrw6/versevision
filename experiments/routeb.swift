import Cocoa
import ApplicationServices
func now() -> Double { Date().timeIntervalSince1970 }
func attr(_ e: AXUIElement, _ a: String) -> AnyObject? { var v: CFTypeRef?; return AXUIElementCopyAttributeValue(e, a as CFString, &v) == .success ? (v as AnyObject?) : nil }
func kids(_ e: AXUIElement) -> [AXUIElement] { (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] }
func s(_ e: AXUIElement, _ a: String) -> String { (attr(e, a) as? String) ?? "" }
func frame(_ e: AXUIElement) -> CGRect? {
    guard let p = attr(e, kAXPositionAttribute), let z = attr(e, kAXSizeAttribute) else { return nil }
    var pt = CGPoint.zero, sz = CGSize.zero; AXValueGetValue(p as! AXValue, .cgPoint, &pt); AXValueGetValue(z as! AXValue, .cgSize, &sz); return CGRect(origin: pt, size: sz)
}
func find(_ e: AXUIElement, _ d: Int, _ p: (AXUIElement) -> Bool) -> AXUIElement? { if p(e) { return e }; if d > 16 { return nil }; for c in kids(e) { if let r = find(c, d + 1, p) { return r } }; return nil }
func findAll(_ e: AXUIElement, _ d: Int, _ p: (AXUIElement) -> Bool, _ out: inout [AXUIElement]) { if p(e) { out.append(e) }; if d > 16 { return }; for c in kids(e) { findAll(c, d + 1, p, &out) } }
func click(_ p: CGPoint) { for t in [CGEventType.leftMouseDown, .leftMouseUp] { CGEvent(mouseEventSource: nil, mouseType: t, mouseCursorPosition: p, mouseButton: .left)?.post(tap: .cghidEventTap) } }
func key(_ code: CGKeyCode) { for d in [true, false] { CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: d)?.post(tap: .cghidEventTap) } }
func typeText(_ t: String) { for ch in t.utf16 { var c = ch; for d in [true, false] { let ev = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: d); ev?.keyboardSetUnicodeString(stringLength: 1, unicodeString: &c); ev?.post(tap: .cghidEventTap) } } }
func api(_ path: String) -> [String: Any]? { guard let u = URL(string: "http://127.0.0.1:1025" + path), let d = try? Data(contentsOf: u) else { return nil }; return (try? JSONSerialization.jsonObject(with: d)) as? [String: Any] }
func liveText() -> String {
    guard let a = api("/v1/presentation/active"), let p = a["presentation"] as? [String: Any], let g = (p["groups"] as? [[String: Any]])?.first,
          let sl = (g["slides"] as? [[String: Any]])?.first else { return "" }
    return (g["name"] as? String ?? "") + " | " + (sl["text"] as? String ?? "")
}
let args = CommandLine.arguments
let ref = args[1]; let mode = args.count > 2 ? args[2] : "add"        // add = copy to presentation then trigger it; live = click result
let app = NSRunningApplication.runningApplications(withBundleIdentifier: "com.renewedvision.propresenter").first!
let ax = AXUIElementCreateApplication(app.processIdentifier)
let win = (attr(ax, kAXWindowsAttribute) as! [AXUIElement])[0]
let t0 = now(); func lap(_ l: String) { print(String(format: "%6.0f ms  %@", (now() - t0) * 1000, l)) }
var fields: [AXUIElement] = []
findAll(win, 0, { s($0, kAXRoleAttribute) == "AXTextField" && s($0, kAXSubroleAttribute) != "AXSearchField" && (frame($0).map { $0.width > 240 && $0.minX > 250 && $0.minX < 300 && $0.minY > 90 && $0.minY < 120 } ?? false) }, &fields)
guard let lookup = fields.first else { print("no lookup field"); exit(2) }
AXUIElementSetAttributeValue(lookup, kAXFocusedAttribute as CFString, kCFBooleanTrue)
AXUIElementSetAttributeValue(lookup, kAXValueAttribute as CFString, "" as CFString)
typeText(ref); key(36); lap("typed + return")
var scrolls: [AXUIElement] = []
findAll(win, 0, { s($0, kAXRoleAttribute) == "AXScrollArea" && (frame($0).map { $0.width > 1200 && $0.height > 800 } ?? false) }, &scrolls)
guard let first = scrolls.first, let r = frame(first) else { print("no results area"); exit(3) }
usleep(40_000)
let thumb = CGPoint(x: r.minX + 333, y: r.minY + 190)
if mode == "add" {
    // select the result, then Save As… > Copy to Selected Presentation
    click(thumb); lap("selected result (this also triggers it)")
    guard let mb = find(win, 0, { s($0, kAXRoleAttribute) == "AXMenuButton" && (s($0, kAXDescriptionAttribute) == "Save As…" || s($0, kAXTitleAttribute) == "Save As…") }) else { print("no Save As button"); exit(5) }
    AXUIElementPerformAction(mb, kAXPressAction as CFString); lap("opened Save As… menu")
    var item: AXUIElement? = nil; let dl = now() + 2
    while item == nil && now() < dl { item = find(mb, 0, { s($0, kAXRoleAttribute) == "AXMenuItem" && s($0, kAXTitleAttribute) == "Copy to Selected Presentation" }); if item == nil { usleep(10_000) } }
    guard let it = item else { print("menu item not found"); exit(4) }
    AXUIElementPerformAction(it, kAXPressAction as CFString); lap("copied to selected presentation")
} else { click(thumb); lap("clicked result") }
let dl = now() + 4
while now() < dl { let t = liveText(); if t.contains(ref.split(separator: ":").first.map(String.init) ?? ref) { lap("live: " + t.prefix(40)); break }; usleep(15_000) }
