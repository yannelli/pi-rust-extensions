import AppKit
import CoreGraphics
import Foundation

let keyCodes: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11,
    "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21,
    "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29, "]": 30, "o": 31,
    "u": 32, "[": 33, "i": 34, "p": 35, "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42,
    ",": 43, "/": 44, "n": 45, "m": 46, ".": 47, "`": 50,
    "return": 36, "enter": 36, "tab": 48, "space": 49, "backspace": 51, "delete": 51,
    "escape": 53, "esc": 53, "forwarddelete": 117, "home": 115, "end": 119, "pageup": 116,
    "pagedown": 121, "left": 123, "right": 124, "down": 125, "up": 126,
    "arrowleft": 123, "arrowright": 124, "arrowdown": 125, "arrowup": 126,
    "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97, "f7": 98, "f8": 100,
    "f9": 101, "f10": 109, "f11": 103, "f12": 111,
]

func modifier(_ key: String) -> CGEventFlags? {
    switch key {
    case "cmd", "command", "meta", "super": return .maskCommand
    case "shift": return .maskShift
    case "alt", "option", "opt": return .maskAlternate
    case "ctrl", "control": return .maskControl
    case "fn": return .maskSecondaryFn
    default: return nil
    }
}

func number(_ value: Any?) -> Double {
    (value as? NSNumber)?.doubleValue ?? 0
}

func point(_ step: JSON) -> CGPoint {
    CGPoint(x: number(step["x"]), y: number(step["y"]))
}

func post(_ event: CGEvent?) throws {
    guard let event else { throw HelperError(message: "Could not create input event") }
    event.post(tap: .cghidEventTap)
}

func mouseTypes(_ button: String) -> (CGEventType, CGEventType, CGMouseButton) {
    switch button {
    case "right": return (.rightMouseDown, .rightMouseUp, .right)
    case "middle": return (.otherMouseDown, .otherMouseUp, .center)
    default: return (.leftMouseDown, .leftMouseUp, .left)
    }
}

func click(_ step: JSON) throws {
    let at = point(step)
    let (down, up, button) = mouseTypes(step["button"] as? String ?? "left")
    let count = max(1, min(3, Int(number(step["count"]))))
    try post(CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: at, mouseButton: button))
    for index in 1...count {
        for type in [down, up] {
            let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: at, mouseButton: button)
            event?.setIntegerValueField(.mouseEventClickState, value: Int64(index))
            try post(event)
        }
    }
}

func drag(_ step: JSON) throws {
    let points = ((step["path"] as? [JSON]) ?? []).map(point)
    guard let first = points.first, points.count >= 2 else {
        throw HelperError(message: "drag.path must contain at least two points.")
    }
    var last = first
    defer {
        CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: last, mouseButton: .left)?
            .post(tap: .cghidEventTap)
    }
    try post(CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: first, mouseButton: .left))
    for next in points.dropFirst() {
        usleep(16_000)
        last = next
        try post(CGEvent(mouseEventSource: nil, mouseType: .leftMouseDragged, mouseCursorPosition: next, mouseButton: .left))
    }
}

func release() {
    let at = CGEvent(source: nil)?.location ?? .zero
    CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: at, mouseButton: .left)?
        .post(tap: .cghidEventTap)
    let flags = CGEvent(source: nil)
    flags?.type = .flagsChanged
    flags?.flags = []
    flags?.post(tap: .cghidEventTap)
}

func scroll(_ step: JSON) throws {
    try post(CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: point(step), mouseButton: .left))
    let dy = Int32(-number(step["dy"]))
    let dx = Int32(-number(step["dx"]))
    try post(CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2, wheel1: dy, wheel2: dx, wheel3: 0))
}

func typeText(_ text: String) throws {
    let units = Array(text.utf16)
    var start = 0
    while start < units.count {
        let chunk = Array(units[start..<min(start + 16, units.count)])
        for keyDown in [true, false] {
            let event = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: keyDown)
            chunk.withUnsafeBufferPointer { buffer in
                event?.keyboardSetUnicodeString(stringLength: buffer.count, unicodeString: buffer.baseAddress)
            }
            try post(event)
        }
        start += 16
        usleep(8_000)
    }
}

func expandKeys(_ keys: [String]) -> [String] {
    keys.flatMap { key -> [String] in
        let lower = key.trimmingCharacters(in: .whitespaces).lowercased()
        return lower.count > 1 && lower.contains("+") ? lower.split(separator: "+").map(String.init) : [lower]
    }
}

func pressKey(_ name: String, flags: CGEventFlags) throws {
    guard let code = keyCodes[name] else { throw HelperError(message: "Unknown key '\(name)'") }
    let down = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true)
    down?.flags = flags
    defer {
        let up = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)
        up?.flags = flags
        up?.post(tap: .cghidEventTap)
    }
    try post(down)
}

func keypress(_ raw: [String]) throws {
    let keys = expandKeys(raw)
    let mods = keys.dropLast().compactMap(modifier)
    if keys.count >= 2 && mods.count == keys.count - 1, let last = keys.last {
        try pressKey(last, flags: CGEventFlags(mods))
        return
    }
    for key in keys {
        try pressKey(key, flags: [])
        usleep(8_000)
    }
}

func act(_ args: JSON) throws -> JSON {
    let steps = (args["steps"] as? [JSON]) ?? []
    var outcomes: [JSON] = []
    for (index, step) in steps.enumerated() {
        let op = step["op"] as? String ?? ""
        do {
            switch op {
            case "press", "setText", "focus": _ = try semanticStep(op, step, args)
            case "click": try click(step)
            case "move": try post(CGEvent(mouseEventSource: nil, mouseType: .mouseMoved, mouseCursorPosition: point(step), mouseButton: .left))
            case "drag": try drag(step)
            case "scroll": try scroll(step)
            case "type": try typeText(step["text"] as? String ?? "")
            case "keys": try keypress((step["keys"] as? [String]) ?? [])
            case "activate": _ = activate(args)
            case "release": release()
            case "wait": usleep(UInt32(max(0, min(5_000, number(step["ms"])))) * 1_000)
            default: throw HelperError(message: "Unknown action '\(op)'")
            }
            outcomes.append(["index": index, "op": op, "outcome": "worked"])
        } catch let error as HelperError {
            outcomes.append(["index": index, "op": op, "outcome": "didnt", "error": error.message])
            break
        }
        usleep(30_000)
    }
    return ["steps": outcomes]
}
