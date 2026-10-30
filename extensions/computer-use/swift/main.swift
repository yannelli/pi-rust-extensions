import AppKit
import ApplicationServices
import CoreGraphics
import Foundation

struct HelperError: Error {
    let message: String
}

typealias JSON = [String: Any]

func rectJSON(_ rect: CGRect) -> JSON {
    ["x": Double(rect.origin.x), "y": Double(rect.origin.y), "w": Double(rect.size.width), "h": Double(rect.size.height)]
}

func rectArg(_ value: Any?) -> CGRect? {
    guard let dict = value as? JSON else { return nil }
    let num = { (key: String) -> Double in (dict[key] as? NSNumber)?.doubleValue ?? 0 }
    return CGRect(x: num("x"), y: num("y"), width: num("w"), height: num("h"))
}

func permissions() -> JSON {
    ["accessibility": AXIsProcessTrusted(), "screenRecording": CGPreflightScreenCaptureAccess()]
}

func registerPermissions() -> JSON {
    let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
    _ = AXIsProcessTrustedWithOptions([key: true] as CFDictionary)
    if !CGPreflightScreenCaptureAccess() {
        _ = CGRequestScreenCaptureAccess()
    }
    return permissions()
}

func listWindows() -> JSON {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    let raw = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [JSON] ?? []
    let frontPid = NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1
    let screen = NSScreen.main
    let scale = Double(screen?.backingScaleFactor ?? 2)
    var seenFront = false
    var windows: [JSON] = []
    for info in raw {
        let layer = (info[kCGWindowLayer as String] as? NSNumber)?.intValue ?? 0
        guard layer == 0 else { continue }
        guard let pid = (info[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value else { continue }
        guard let number = (info[kCGWindowNumber as String] as? NSNumber)?.intValue else { continue }
        guard let boundsDict = info[kCGWindowBounds as String] as? NSDictionary,
              let bounds = CGRect(dictionaryRepresentation: boundsDict as CFDictionary),
              bounds.width > 1, bounds.height > 1 else { continue }
        let app = NSRunningApplication(processIdentifier: pid)
        let focused = pid == frontPid && !seenFront
        if focused { seenFront = true }
        windows.append([
            "pid": Int(pid),
            "windowId": number,
            "app": (info[kCGWindowOwnerName as String] as? String) ?? app?.localizedName ?? "Unknown App",
            "bundleId": app?.bundleIdentifier ?? NSNull(),
            "windowTitle": (info[kCGWindowName as String] as? String) ?? "",
            "framePoints": rectJSON(bounds),
            "scaleFactor": scale,
            "isOnscreen": (info[kCGWindowIsOnscreen as String] as? NSNumber)?.boolValue ?? true,
            "isMinimized": false,
            "isFocused": focused,
        ])
    }
    return ["windows": windows, "frontmostPid": Int(frontPid)]
}

func openPane(_ kind: String) throws -> JSON {
    let anchor = kind == "screenRecording" ? "Privacy_ScreenCapture" : "Privacy_Accessibility"
    guard let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(anchor)") else {
        throw HelperError(message: "invalid settings url")
    }
    return ["opened": NSWorkspace.shared.open(url)]
}

func activate(_ args: JSON) -> JSON {
    let pid = (args["pid"] as? NSNumber)?.int32Value ?? 0
    let app = NSRunningApplication(processIdentifier: pid)
    return ["activated": app?.activate(options: []) ?? false]
}

func run(_ command: String, _ args: JSON) throws -> JSON {
    switch command {
    case "check": return permissions()
    case "register": return registerPermissions()
    case "windows": return listWindows()
    case "openPane": return try openPane(args["kind"] as? String ?? "")
    case "activate": return activate(args)
    case "outline": return try outline(args)
    case "act": return try act(args)
    case "version": return ["version": 1]
    default: throw HelperError(message: "Unknown command '\(command)'")
    }
}

func emit(_ value: JSON) {
    let data = (try? JSONSerialization.data(withJSONObject: value, options: [])) ?? Data("{}".utf8)
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

let argv = CommandLine.arguments
let command = argv.count > 1 ? argv[1] : "version"
let rawArgs = argv.count > 2 ? argv[2] : "{}"
do {
    let parsed = try JSONSerialization.jsonObject(with: Data(rawArgs.utf8), options: [])
    emit(["ok": true, "result": try run(command, parsed as? JSON ?? [:])])
} catch let error as HelperError {
    emit(["ok": false, "error": error.message])
    exit(1)
} catch {
    emit(["ok": false, "error": String(describing: error)])
    exit(1)
}
