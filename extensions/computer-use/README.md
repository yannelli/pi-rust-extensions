# computer-use

Created 2026-10-30. Last updated 2026-10-30.

This extension lets the agent see and operate desktop windows and a managed Chrome or Helium browser. It is a Rust port of [@injaneity/pi-computer-use](https://github.com/injaneity/pi-computer-use) 0.5.1. The tool names, parameter names, refs, and `details` fields are the same as upstream.

## Tools

| Tool | Parameters | Use |
| --- | --- | --- |
| `find_roots` | `text`, `app`, `bundleId`, `pid`, `kind` (all optional) | Lists a maximum of 20 ranked roots (`@rN`) with geometry and focus. |
| `observe_ui` | `root` (`@rN`, optional), `mode` (`semantic`, `visual`, `fused`; default `fused`) | Captures the root or the focused window. Returns an outline (`@eN` refs), a `stateId`, and a JPEG image. `semantic` sends no image. |
| `search_ui` | `stateId`, `text`, `role`, `capability` | Ranks matches in the saved outline: exact, prefix, substring, then fuzzy. Returns a maximum of 12. |
| `expand_ui` | `stateId`, `ref`, `depth` (1 to 8, default 3) | Shows the subtree of one ref. |
| `inspect_ui` | `stateId`, `ref` | Shows the fields, frame, actions, and capabilities of one ref. |
| `act_ui` | `stateId`, `actions` (1 to 20), `expect` (optional) | Runs `press`, `click`, `setText`, `typeText`, `keypress`, `scroll`, `drag`, and `moveMouse`. Returns the next state and the outline changes. The first desktop `act_ui` of a session asks for confirmation; without a UI, desktop `act_ui` is refused. |
| `read_text` | `ref`, `offset`, `stateId` | Reads one page of `@e` text (4000 characters) or of `@o` output (16 KiB). An `@o` ref needs no `stateId`. |
| `wait_for` | `stateId`, `ref` or `scopeRef`, `text`, `role`, `value`, `until`, `timeoutMs` | Observes again every 500 ms until the condition is true or the time limit (default 10000 ms) ends. |
| `launch_browser` | `url` (optional) | Starts the managed browser with a DevTools port, or uses the browser that runs. Returns an observed `browser_page` state. |
| `navigate_browser` | `stateId`, `url` | Opens an HTTP or HTTPS URL in the page of a browser state. |
| `evaluate_browser` | `stateId`, `expression` (a maximum of 64 KiB) | Runs JavaScript in the page of a browser state. The value goes after the outline. |

The tools that read a saved state (`search_ui`, `expand_ui`, `inspect_ui`, `read_text`) do not start a helper and do not call pi. An unknown `stateId` gives this error: `State '<id>' is unavailable or was evicted. Observe the root again.`

The model text has a limit of 48 KiB and 2000 lines. The extension keeps the full text as an `@oN` ref, and the text tells the model the offset for `read_text`.

`click` and `drag` coordinates are window screenshot pixels of the latest image. A browser state uses the pixels of the page screenshot.

## Command

| Command | Use |
| --- | --- |
| `/computer-use` | Loads the configuration again and shows the values and the source files. |
| `/computer-use setup` | Does a check of the tools and permissions now. Opens the permission menu if a permission is missing. |

## Setup and permissions

The first tool that operates the desktop starts setup.

- macOS: the extension builds a Swift helper with `xcrun swiftc` one time. The binary goes in `<agentDir>/ext/computer-use/helper-<hash>/`. If `swiftc` is missing, the menu can start `xcode-select --install`.
- macOS: the helper does a check of Accessibility and Screen Recording. If a permission is missing, the extension asks macOS to show its prompts. Then it shows a menu with "Open Accessibility Settings (missing)", "Open Screen Recording Settings (missing)", "Recheck (restarts helper)", and "Cancel". macOS gives the permission to the app that runs pi, for example your terminal.
- Linux: the extension needs `xdotool`. For screenshots it uses `grim` (Wayland), `maim`, or ImageMagick `import`.
- If pi has no UI and a permission is missing, the tool gives an error that tells you to start pi in interactive mode.

## Platforms

| Platform | Windows and outline | Screenshots | Input |
| --- | --- | --- | --- |
| macOS | `CGWindowList` and the Accessibility tree, through the helper | `screencapture -l<windowId>`, then `sips` to JPEG (a maximum of 1280 pixels) | Accessibility actions (`press`, `setText`, `focus`) and `CGEvent` pointer and keyboard events |
| Linux | `xdotool`; the outline is one `pictureOnly` window node | `grim`, `maim`, or `import` | `xdotool` |
| Windows | Not supported | Not supported | Not supported |
| Managed browser (all platforms) | DOM outline through the DevTools Protocol | `Page.captureScreenshot` | DevTools `Input.*` events and element `click`, `focus`, and value set |

If an action fails or stops, the extension releases the mouse button and the modifier keys.

## Configuration

Global file: `<agentDir>/ext/computer-use.json`. Project file: `<cwd>/.pi/ext/computer-use.json`. The project file sets values key by key over the global file. The extension reads the project file only for a trusted project. Environment variables set values over the two files. A file can also put the keys in a `computer_use` object.

| Key | Default | Environment variable | Use |
| --- | --- | --- | --- |
| `browser_use` | `true` | `PI_COMPUTER_USE_BROWSER_USE` | When `false`, the extension refuses known browser windows and `launch_browser`. |
| `managed_browser` | `"chrome"` | `PI_COMPUTER_USE_MANAGED_BROWSER` | `"chrome"` or `"helium"` for `launch_browser`. |
| `headless` | `false` | `PI_COMPUTER_USE_HEADLESS` | When `true`, desktop `act_ui` accepts only `press` and `setText` on Accessibility refs. Browser actions use DevTools events and stay available. |
| `cursor_overlay` | `true` | `PI_COMPUTER_USE_CURSOR_OVERLAY` | The extension sends pointer input in the foreground. The system cursor moves, and the extension draws no overlay. `/computer-use` shows this. |

A boolean value can be `true` or `false`, `1` or `0`, `yes` or `no`, `on` or `off`, and `enabled` or `disabled`.

Other environment variables:

- `PI_COMPUTER_USE_CDP_PORT`: the DevTools port. If no port is set, the extension finds a free local port.
- `PI_COMPUTER_USE_CHROME_EXECUTABLE`, `PI_COMPUTER_USE_HELIUM_EXECUTABLE`: the browser binary.

## Paths

- State: `<agentDir>/ext/computer-use/` (helper builds, `browser-chrome/` and `browser-helium/` profiles).
- Temporary screenshots: `<agentDir>/ext/computer-use/captures/` (mode 0700). The extension deletes each file after it reads it.

## Session replay

At `session_start`, `session_tree`, and `session_compact`, the extension reads `session.getBranch`. It makes the `@r` roots and the saved states again from the `details` of the tool results. The newest state of each window stays. The store keeps a maximum of 128 states and 64 `@o` outputs.

When pi shuts down the extension, the extension stops the browser that it started.

## Differences from upstream

- The macOS helper is a Swift program that the extension builds on your Mac. Upstream ships a signed `pi-computer-use.app`.
- The extension does no OCR. `visual` and `fused` both send the screenshot.
- Linux has no AT-SPI outline. Use coordinate actions from the screenshot.
- Windows is not supported.
- The extension does not draw a cursor overlay.
- `act_ui` does not try foreground keyboard input again after a failed background attempt.
- Config paths follow the pi-rust convention (`ext/computer-use.json`). Upstream uses `extensions/pi-computer-use.json` and `.pi/computer-use.json`.
