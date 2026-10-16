import Cocoa
import ApplicationServices

// VerseVision helper: answer ProPresenter's "Are you sure you want to quit?" box.
// usage: vvdialog <app name> <text that must appear in the dialog> <button to press>
// Only presses the named button inside a dialog/sheet that contains the given text.
// Prints one of: PRESSED | NO_DIALOG | NO_BUTTON | NOT_RUNNING
let args = CommandLine.arguments
guard args.count >= 4 else { print("usage: vvdialog APP TEXT BUTTON"); exit(2) }
let appName = args[1], needle = args[2], button = args[3]

guard let app = NSWorkspace.shared.runningApplications.first(where: { $0.localizedName == appName }) else { print("NOT_RUNNING"); exit(0) }
let root = AXUIElementCreateApplication(app.processIdentifier)

func attr(_ e: AXUIElement, _ a: String) -> AnyObject? {
    var v: CFTypeRef?
    AXUIElementCopyAttributeValue(e, a as CFString, &v)
    return v
}
func str(_ e: AXUIElement, _ a: String) -> String { (attr(e, a) as? String) ?? "" }
func kids(_ e: AXUIElement) -> [AXUIElement] { (attr(e, kAXChildrenAttribute as String) as? [AXUIElement]) ?? [] }

func contains(_ e: AXUIElement, _ text: String, _ depth: Int = 0) -> Bool {
    if depth > 12 { return false }
    for a in [kAXTitleAttribute, kAXValueAttribute, kAXDescriptionAttribute] {
        if str(e, a as String).localizedCaseInsensitiveContains(text) { return true }
    }
    return kids(e).contains { contains($0, text, depth + 1) }
}
func findButton(_ e: AXUIElement, _ name: String, _ depth: Int = 0) -> AXUIElement? {
    if depth > 12 { return nil }
    if str(e, kAXRoleAttribute as String) == "AXButton" && str(e, kAXTitleAttribute as String) == name { return e }
    for k in kids(e) { if let b = findButton(k, name, depth + 1) { return b } }
    return nil
}
// Dialog containers: sheets and dialog windows (not the main document window).
func dialogs(_ e: AXUIElement, _ depth: Int = 0) -> [AXUIElement] {
    if depth > 8 { return [] }
    var out: [AXUIElement] = []
    let role = str(e, kAXRoleAttribute as String), sub = str(e, kAXSubroleAttribute as String)
    if role == "AXSheet" || sub == "AXDialog" || sub == "AXSystemDialog" || role == "AXDialog" { out.append(e) }
    for k in kids(e) { out += dialogs(k, depth + 1) }
    return out
}
var found = false
for d in dialogs(root) where contains(d, needle) {
    found = true
    if let b = findButton(d, button) {
        AXUIElementPerformAction(b, kAXPressAction as CFString)
        print("PRESSED")
        exit(0)
    }
}
print(found ? "NO_BUTTON" : "NO_DIALOG")
