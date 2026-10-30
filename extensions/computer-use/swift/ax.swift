import ApplicationServices
import CoreGraphics
import Foundation

func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    let status = AXUIElementCopyAttributeValue(element, name as CFString, &value)
    return status == .success ? value : nil
}

func stringAttribute(_ element: AXUIElement, _ name: String) -> String? {
    guard let value = attribute(element, name) else { return nil }
    if let text = value as? String { return text.isEmpty ? nil : text }
    if let number = value as? NSNumber { return number.stringValue }
    return nil
}

func children(_ element: AXUIElement) -> [AXUIElement] {
    guard let value = attribute(element, kAXChildrenAttribute as String) else { return [] }
    return (value as? [AXUIElement]) ?? []
}

func frame(_ element: AXUIElement) -> CGRect? {
    guard let position = attribute(element, kAXPositionAttribute as String),
          let size = attribute(element, kAXSizeAttribute as String),
          CFGetTypeID(position) == AXValueGetTypeID(),
          CFGetTypeID(size) == AXValueGetTypeID() else { return nil }
    var point = CGPoint.zero
    var extent = CGSize.zero
    guard AXValueGetValue(position as! AXValue, .cgPoint, &point),
          AXValueGetValue(size as! AXValue, .cgSize, &extent) else { return nil }
    return CGRect(origin: point, size: extent)
}

func actionNames(_ element: AXUIElement) -> [String] {
    var names: CFArray?
    guard AXUIElementCopyActionNames(element, &names) == .success else { return [] }
    return (names as? [String]) ?? []
}

func settable(_ element: AXUIElement, _ name: String) -> Bool {
    var flag = DarwinBoolean(false)
    guard AXUIElementIsAttributeSettable(element, name as CFString, &flag) == .success else { return false }
    return flag.boolValue
}

func distance(_ a: CGRect, _ b: CGRect) -> Double {
    Double(abs(a.minX - b.minX) + abs(a.minY - b.minY) + abs(a.width - b.width) + abs(a.height - b.height))
}

func window(_ args: JSON) throws -> AXUIElement {
    guard let pid = (args["pid"] as? NSNumber)?.int32Value else {
        throw HelperError(message: "pid is required")
    }
    let app = AXUIElementCreateApplication(pid)
    _ = AXUIElementSetMessagingTimeout(app, 2.0)
    let listed = (attribute(app, kAXWindowsAttribute as String) as? [AXUIElement]) ?? []
    let real = listed.filter { stringAttribute($0, kAXRoleAttribute as String) == (kAXWindowRole as String) }
    let windows = real.isEmpty ? listed : real
    guard !windows.isEmpty else {
        throw HelperError(message: "No accessible window for pid \(pid). Grant Accessibility and retry.")
    }
    let wanted = rectArg(args["frame"])
    let title = args["windowTitle"] as? String
    let ranked = windows.map { candidate -> (AXUIElement, Double) in
        var score = wanted.flatMap { w in frame(candidate).map { distance($0, w) } } ?? 1e9
        if let title, !title.isEmpty, stringAttribute(candidate, kAXTitleAttribute as String) == title { score -= 0.5 }
        return (candidate, score)
    }
    return ranked.min { $0.1 < $1.1 }!.0
}

func resolve(_ args: JSON) throws -> AXUIElement {
    var element = try window(args)
    for index in (args["path"] as? [NSNumber]) ?? [] {
        let list = children(element)
        guard index.intValue < list.count else {
            throw HelperError(message: "Element path is stale. Observe the root again.")
        }
        element = list[index.intValue]
    }
    return element
}

struct WalkLimits {
    let maxDepth: Int
    let maxNodes: Int
    var count = 0
}

func node(_ element: AXUIElement, path: [Int], depth: Int, limits: inout WalkLimits) -> JSON {
    limits.count += 1
    let role = stringAttribute(element, kAXRoleAttribute as String) ?? "AXUnknown"
    let actions = actionNames(element)
    let label = stringAttribute(element, kAXTitleAttribute as String)
        ?? stringAttribute(element, kAXDescriptionAttribute as String)
        ?? stringAttribute(element, kAXHelpAttribute as String)
    var json: JSON = [
        "ref": "",
        "role": role,
        "actions": actions,
        "canPress": actions.contains(kAXPressAction as String),
        "canSetValue": settable(element, kAXValueAttribute as String),
        "canFocus": settable(element, kAXFocusedAttribute as String),
        "canScroll": role == (kAXScrollAreaRole as String),
        "focused": (attribute(element, kAXFocusedAttribute as String) as? Bool) ?? false,
        "path": path,
    ]
    if let label { json["label"] = label }
    if let value = stringAttribute(element, kAXValueAttribute as String) { json["value"] = value }
    if let identifier = stringAttribute(element, kAXIdentifierAttribute as String) { json["identifier"] = identifier }
    if let rect = frame(element) { json["frame"] = rectJSON(rect) }
    var kids: [JSON] = []
    if depth < limits.maxDepth {
        for (index, child) in children(element).enumerated() where limits.count < limits.maxNodes {
            kids.append(node(child, path: path + [index], depth: depth + 1, limits: &limits))
        }
    }
    json["children"] = kids
    return json
}

func outline(_ args: JSON) throws -> JSON {
    guard AXIsProcessTrusted() else {
        throw HelperError(message: "Accessibility permission is missing. Run /computer-use setup.")
    }
    let root = try window(args)
    var limits = WalkLimits(
        maxDepth: (args["maxDepth"] as? NSNumber)?.intValue ?? 24,
        maxNodes: (args["maxNodes"] as? NSNumber)?.intValue ?? 1500
    )
    let tree = node(root, path: [], depth: 0, limits: &limits)
    return ["root": tree, "nodes": limits.count, "truncated": limits.count >= limits.maxNodes]
}

func semanticStep(_ op: String, _ step: JSON, _ base: JSON) throws -> String {
    var args = base
    args["path"] = step["path"]
    let element = try resolve(args)
    let status: AXError
    switch op {
    case "press":
        status = AXUIElementPerformAction(element, kAXPressAction as CFString)
    case "setText":
        let text = (step["text"] as? String) ?? ""
        status = AXUIElementSetAttributeValue(element, kAXValueAttribute as CFString, text as CFString)
    case "focus":
        status = AXUIElementSetAttributeValue(element, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    default:
        throw HelperError(message: "Unknown semantic action '\(op)'")
    }
    guard status == .success else {
        throw HelperError(message: "\(op) failed with AXError \(status.rawValue)")
    }
    return "worked"
}
