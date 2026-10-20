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
func api(_ path: String) -> [String: Any]? { guard let u = URL(string: "http://127.0.0.1:1025" + path), let d = try? Data(contentsOf: u) else { return nil }; return (try? JSONSerialization.jsonObject(with: d)) as? [String: Any] }

let args = Array(CommandLine.arguments.dropFirst())
var mode = "add"; var refs: [String] = []; var trace = false; var verifyEach = false; var waitMenu = false; var showSeconds = 0.0; var keepOnScreen = false; var alsoAdd = false; var backToShow = false; var wantVersion = ""
for a in args { if a == "--live" { mode = "live" } else if a == "--trace" { trace = true } else if a == "--verify-each" { verifyEach = true } else if a == "--wait-menu" { waitMenu = true } else if a == "--keep" { keepOnScreen = true; mode = "live" } else if a == "--add" { alsoAdd = true } else if a == "--back" { backToShow = true } else if a.hasPrefix("--version=") { wantVersion = String(a.dropFirst(10)) } else if a.hasPrefix("--show=") { showSeconds = Double(a.dropFirst(7)) ?? 10; mode = "live" } else { refs.append(a) } }
guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: "com.renewedvision.propresenter").first else { print("ProPresenter not running"); exit(1) }
let ax = AXUIElementCreateApplication(app.processIdentifier)
// Bring ProPresenter to the front first, because typed keys and clicks go to whatever app is in front.
// The shortcut does `activate` + a fixed 1 s delay; this waits only until it is really frontmost (usually a few ms).
func frontPid() -> pid_t {            // NSWorkspace's value is cached in a command-line tool; accessibility is live
    let sys = AXUIElementCreateSystemWide()
    var v: CFTypeRef?
    guard AXUIElementCopyAttributeValue(sys, kAXFocusedApplicationAttribute as CFString, &v) == .success, let e = v else { return -1 }
    var pid: pid_t = -1; AXUIElementGetPid(e as! AXUIElement, &pid); return pid
}
func bringToFront(_ timeout: Double = 1.5) -> Double {
    let t = Date().timeIntervalSince1970
    if frontPid() == app.processIdentifier { return 0 }
    app.activate(options: [.activateIgnoringOtherApps])
    let dl = t + timeout
    while Date().timeIntervalSince1970 < dl && frontPid() != app.processIdentifier { RunLoop.current.run(until: Date().addingTimeInterval(0.002)) }
    return (Date().timeIntervalSince1970 - t) * 1000
}
let focusMs = bringToFront()
if frontPid() != app.processIdentifier { print("could not bring ProPresenter to the front"); exit(6) }
if CommandLine.arguments.contains("--activate-only") { print(String(format: "ProPresenter is in front (took %.0f ms)", focusMs)); exit(0) }
let allWins = (attr(ax, kAXWindowsAttribute) as? [AXUIElement]) ?? []
// the main window is the biggest one (other windows can be a welcome or update panel)
guard let win = allWins.max(by: { (frame($0).map { $0.width * $0.height } ?? 0) < (frame($1).map { $0.width * $0.height } ?? 0) }) else { print("no window"); exit(1) }

let winFrame = frame(win) ?? CGRect(x: 0, y: 0, width: 1600, height: 1000)
// The lookup box and the version menu have no label of their own, but their neighbours do. Everything is found from the labelled
// "search" button of the Bible panel and from the lookup box itself, so the window size and sidebar width do not matter.
func parent(_ e: AXUIElement) -> AXUIElement? { attr(e, kAXParentAttribute).map { $0 as! AXUIElement } }
func searchButton() -> AXUIElement? { find(win, 0, { s($0, kAXRoleAttribute) == "AXButton" && s($0, kAXDescriptionAttribute) == "search" }) }
func bibleAreaGroup() -> AXUIElement? {          // the big panel that contains the search button
    var cur = searchButton()
    while let c = cur {
        if s(c, kAXRoleAttribute) == "AXGroup", let f = frame(c), f.height > winFrame.height * 0.5, f.width > winFrame.width * 0.3 { return c }
        cur = parent(c)
    }
    return nil
}
func lookupField() -> AXUIElement? {             // the wide, non-search text box on the same row as the search button, to its left
    guard let sb = searchButton(), let sf = frame(sb) else { return nil }
    var f: [AXUIElement] = []
    findAll(win, 0, { e in s(e, kAXRoleAttribute) == "AXTextField" && s(e, kAXSubroleAttribute) != "AXSearchField" && (frame(e).map { $0.width > 150 && abs($0.midY - sf.midY) < 14 && $0.maxX <= sf.minX + 5 } ?? false) }, &f)
    return f.max(by: { (frame($0)?.maxX ?? 0) < (frame($1)?.maxX ?? 0) })      // the closest one to the left of the search box (sidebar names are further left)
}
// Open Bible mode (the 4th toolbar button; the shortcut clicks button 1 then 4 to force a refresh). Only if it is not already open.
if lookupField() == nil {
    let bible = find(win, 0, { s($0, kAXRoleAttribute) == "AXRadioButton" && s($0, kAXDescriptionAttribute) == "Bible" })
    guard let b = bible else { print("could not find the Bible toolbar button"); exit(5) }
    AXUIElementPerformAction(b, kAXPressAction as CFString)
    let dl0 = now() + 2; while lookupField() == nil && now() < dl0 { usleep(10_000) }
}
// find the controls ONCE (the original script re-walks a ten-level path for every click)
var fields: [AXUIElement] = []
if let lf = lookupField() { fields = [lf] }
guard let lookup = fields.first else { print("lookup field not found - is Bible mode open?"); exit(2) }
// Bible version: pick it in the dropdown under the lookup field when one was asked for. An unknown name leaves it alone.
func versionPopup() -> AXUIElement? {
    guard let lf = frame(lookup) else { return nil }
    var pops: [AXUIElement] = []
    findAll(win, 0, { e in s(e, kAXRoleAttribute) == "AXPopUpButton" && (frame(e).map { abs($0.minX - lf.minX) < 25 && $0.minY > lf.maxY - 5 && $0.minY < lf.maxY + 70 && $0.width > 150 } ?? false) }, &pops)
    return pops.first
}
var versionNote = ""
if !wantVersion.isEmpty, let pop = versionPopup() {
    let tv = now(); let was = s(pop, kAXValueAttribute)
    if was == wantVersion { versionNote = "version already \(wantVersion)" } else {
        AXUIElementPerformAction(pop, kAXPressAction as CFString)
        var items: [AXUIElement] = []; let dlv = now() + 1.0
        while items.isEmpty && now() < dlv { findAll(pop, 0, { s($0, kAXRoleAttribute) == "AXMenuItem" }, &items); if items.isEmpty { usleep(3_000) } }
        if let it = items.first(where: { s($0, kAXTitleAttribute) == wantVersion }) {
            AXUIElementPerformAction(it, kAXPressAction as CFString)
            let dlw = now() + 1.0; while s(pop, kAXValueAttribute) != wantVersion && now() < dlw { usleep(5_000) }
            versionNote = s(pop, kAXValueAttribute) == wantVersion ? String(format: "version %@ -> %@ (%.0f ms)", was, wantVersion, (now() - tv) * 1000) : "could not switch the version (still \(s(pop, kAXValueAttribute)))"
        } else { key(53); usleep(30_000); versionNote = "version '\(wantVersion)' is not in ProPresenter, left on \(was)" }
    }
    print("  " + versionNote)
}
guard let area = bibleAreaGroup() else { print("bible area not found"); exit(3) }
let areaFrame = frame(area) ?? winFrame
guard let saveAs = find(win, 0, { s($0, kAXRoleAttribute) == "AXMenuButton" && (s($0, kAXDescriptionAttribute) == "Save As…" || s($0, kAXTitleAttribute) == "Save As…") }) else { print("Save As not found"); exit(4) }
var scrolls: [AXUIElement] = []
findAll(area, 0, { s($0, kAXRoleAttribute) == "AXScrollArea" && (frame($0).map { $0.width > areaFrame.width * 0.6 && $0.height > winFrame.height * 0.4 } ?? false) }, &scrolls)
let results = scrolls.first.flatMap { frame($0) }
if trace { print("  [lookup \(frame(lookup).map { "\($0)" } ?? "?"), area \(areaFrame), results \(results.map { "\($0)" } ?? "none"), scrolls \(scrolls.count)]") }
func busy() -> Bool {                      // while a lookup runs ProPresenter shows a small "stop progress" button
    for b in kids(area) where s(b, kAXRoleAttribute) == "AXButton" && s(b, kAXDescriptionAttribute) == "stop progress" { return true }
    return false
}
func hasOptions() -> Bool { kids(area).contains { s($0, kAXRoleAttribute) == "AXButton" && s($0, kAXTitleAttribute) == "Options" } }
func typeText(_ t: String) { for ch in t.utf16 { var c = ch; for d in [true, false] { let ev = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: d); ev?.keyboardSetUnicodeString(stringLength: 1, unicodeString: &c); ev?.post(tap: .cghidEventTap) } } }

func menuOpen() -> Bool { kids(saveAs).contains { s($0, kAXRoleAttribute) == "AXMenu" } }
func lastGroupName(_ uuid: String) -> String {
    guard let a = api("/v1/presentation/" + uuid), let p = a["presentation"] as? [String: Any], let g = (p["groups"] as? [[String: Any]])?.last else { return "" }
    return g["name"] as? String ?? ""
}
func norm(_ x: String) -> String { x.lowercased().replacingOccurrences(of: "psalm ", with: "psalms ").replacingOccurrences(of: " ", with: "") }
var target = ""            // the presentation the verses are copied into (the selected one)
if let f = api("/v1/presentation/focused"), let u = f["uuid"] as? String { target = u }
var baseline = 0
if !target.isEmpty, let a = api("/v1/presentation/" + target), let p = a["presentation"] as? [String: Any], let gs = p["groups"] as? [[String: Any]] { baseline = gs.count }

func copyToSelected(_ m: (String) -> Void) -> Bool {
    for attempt in 0..<3 {
        AXUIElementPerformAction(saveAs, kAXPressAction as CFString)
        var item: AXUIElement? = nil; let d2 = now() + 0.8
        while item == nil && now() < d2 { item = find(saveAs, 0, { s($0, kAXRoleAttribute) == "AXMenuItem" && s($0, kAXTitleAttribute) == "Copy to Selected Presentation" }); if item == nil { usleep(3_000) } }
        if let it = item { AXUIElementPerformAction(it, kAXPressAction as CFString); m("copied"); return true }
        key(53); usleep(60_000); m("retry\(attempt + 1)")
    }
    return false
}
func goBackToShow(_ m: (String) -> Void) {
    if let b = find(win, 0, { s($0, kAXRoleAttribute) == "AXRadioButton" && s($0, kAXDescriptionAttribute) == "Show" }) { AXUIElementPerformAction(b, kAXPressAction as CFString); m("back to Show") }
}
func liveLayer() -> Bool { (api("/v1/status/layers")?["slide"] as? Bool) ?? false }
func activeName() -> String {
    guard let a = api("/v1/presentation/active"), let p = a["presentation"] as? [String: Any], let g = (p["groups"] as? [[String: Any]])?.first else { return "" }
    return g["name"] as? String ?? ""
}
let T0 = now()
var ok = 0, failed: [String] = [], wrong: [String] = []
for ref in refs {
    let t0 = now(); var marks: [String] = []
    func m(_ l: String) { marks.append(String(format: "%@ %.0f", l, (now() - t0) * 1000)) }
    if frontPid() != app.processIdentifier { let ms = bringToFront(); m(String(format: "refocus %.0f", ms)) }
    if menuOpen() { key(53); usleep(30_000) }                       // never start with a menu left open
    AXUIElementSetAttributeValue(lookup, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    AXUIElementSetAttributeValue(lookup, kAXValueAttribute as CFString, "" as CFString)
    typeText(ref); key(36); m("typed")
    // The lookup shows a Cancel button while it runs. Wait to SEE it start (so we never mistake the previous
    // verse for the new one), then wait for it to finish. If it never shows (an instant cached result), allow a short floor.
    var started = false; let t1 = now() + 0.30
    while now() < t1 { if busy() { started = true; break }; usleep(1_000) }
    m(started ? "started" : "nostart")
    if started { let t2 = now() + 4; while now() < t2 && busy() { usleep(1_000) } } else { usleep(60_000) }
    m("loaded")
    if mode == "live" && showSeconds == 0 && !keepOnScreen, let r = results { click(CGPoint(x: r.minX + 333, y: r.minY + 190)); m("clicked") }
    if showSeconds > 0 || keepOnScreen {
        // Click the first result, then make sure ProPresenter reports THIS verse as the live one (not just "something is live",
        // which would already be true while a previous verse is still up).
        let want = norm(ref)
        var okLive = false, tries = 0
        let dl5 = now() + 2.5
        while !okLive && now() < dl5 && tries < 6 {
            if let r = results { click(CGPoint(x: r.minX + 333, y: r.minY + 190)); tries += 1; m("click\(tries)") }
            let w = now() + 0.35
            while now() < w { if liveLayer() && norm(activeName()).hasPrefix(want) { okLive = true; break }; usleep(5_000) }
        }
        if !okLive { failed.append(ref); print("  never went live: \(ref) (screen shows '\(activeName())')"); continue }
        let shownAt = (now() - t0) * 1000; m("LIVE")
        var addedNote = ""
        if alsoAdd { addedNote = copyToSelected(m) ? ", added to the presentation" : ", COULD NOT add to the presentation" }
        // Hand the screen back to the operator: the verse is up and saved, so leave Bible mode for Show as soon as possible.
        if backToShow, let b = find(win, 0, { s($0, kAXRoleAttribute) == "AXRadioButton" && s($0, kAXDescriptionAttribute) == "Show" }) {
            AXUIElementPerformAction(b, kAXPressAction as CFString); m("back to Show")
        }
        print(String(format: "  %-22@ live after %4.0f ms (shows: %@)%@", ref as NSString, shownAt, activeName(), addedNote) + (keepOnScreen ? ", staying up" : String(format: ", holding %.0f s", showSeconds)) + (trace ? "   [" + marks.joined(separator: ", ") + "]" : ""))
        if keepOnScreen { ok += 1; continue }
        usleep(UInt32(showSeconds * 1_000_000))
        if let u = URL(string: "http://127.0.0.1:1025/v1/clear/layer/slide") { _ = try? Data(contentsOf: u) }
        let dl6 = now() + 1; while liveLayer() && now() < dl6 { usleep(5_000) }
        print(liveLayer() ? "  could not clear the screen!" : "  cleared")
        ok += 1; continue
    }
    let done = copyToSelected(m)
    if !done { failed.append(ref); print("  FAILED to add \(ref)"); continue }
    if backToShow { goBackToShow(m) }
    if waitMenu { let d3 = now() + 1; while menuOpen() && now() < d3 { usleep(3_000) }; m("menu closed") }
    var good = true
    if verifyEach && !target.isEmpty {
        good = false; let d4 = now() + 1.2
        while now() < d4 { if norm(lastGroupName(target)).hasPrefix(norm(ref.split(separator: "-").first.map(String.init) ?? ref)) { good = true; break }; usleep(10_000) }
        m(good ? "verified" : "MISMATCH")
    }
    if good { ok += 1 } else { wrong.append(ref) }
    print(String(format: "  %-22@ %4.0f ms", ref as NSString, (now() - t0) * 1000) + (good ? "" : "  <-- got \(lastGroupName(target))") + (trace ? "   [" + marks.joined(separator: ", ") + "]" : ""))
}
var finalNote = ""
if !verifyEach && !target.isEmpty && ok > 0 && showSeconds == 0 && !keepOnScreen {      // the add-only batch check; show/keep modes report per verse
    let tv = now(); var names: [String] = []
    let dv = now() + 2.5
    while now() < dv { if let a = api("/v1/presentation/" + target), let p = a["presentation"] as? [String: Any], let gs = p["groups"] as? [[String: Any]] { names = gs.compactMap { $0["name"] as? String }; if names.count >= ok + baseline { break } }; usleep(20_000) }
    let added = Array(names.suffix(ok)); let want = refs.filter { !failed.contains($0) }
    let mism = zip(added, want).filter { norm($0.0) != norm($0.1) }.map { "\($0.1)->\($0.0)" }
    finalNote = String(format: "  final check (%.0f ms): %d slides added, %@", (now() - tv) * 1000, max(0, names.count - baseline), mism.isEmpty ? "all match the references" : "MISMATCH " + mism.joined(separator: ", "))
}
print(finalNote)
print(String(format: "%@: %d/%d verses in %.2f s = %.0f ms each", mode, ok, refs.count, now() - T0, (now() - T0) * 1000 / Double(max(refs.count, 1))))
if !failed.isEmpty { print("failed:", failed) }
if !wrong.isEmpty { print("wrong slide added for:", wrong) }
