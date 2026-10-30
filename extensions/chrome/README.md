# pi-ext-chrome

Created 2026-10-30. Last updated 2026-10-30.

Controls Google Chrome from pi through the Chrome DevTools Protocol (CDP) over a WebSocket. The extension connects to a Chrome that listens on a remote debugging port. If no Chrome answers, it can start Chrome with a managed profile. The tools work only after the user authorizes them for the session.

Tool and parameter names follow the upstream `pi-chrome` package. The upstream package uses a companion Chrome extension and a loopback bridge. This crate uses CDP directly, so it needs no Chrome extension.

## Authorization

- At `session_start`, the extension removes all `chrome_*` tools from the active tool set.
- `/chrome authorize [15m|30m|2h|<minutes>|indefinite]` asks for confirmation, then adds the tools. The default comes from `defaultAuthorizeMinutes`. The maximum is 7 days.
- `/chrome revoke`, or the end of the time limit, removes the tools and closes the tabs that this session opened.
- A tool call in a locked session returns an error and opens no connection.
- Without an interactive UI, `/chrome authorize` fails and the tools stay locked.
- Each change writes a `pi-chrome-tool-change` session entry. After `/reload`, a grant that is still valid comes back only in the session that wrote it.
- While authorized, `before_agent_start` adds the system prompt section `chrome-profile-bridge`.
- The status bar shows `● Chrome · <time> left`.

## Tools

With no `targetId`, `urlIncludes`, or `titleIncludes`, a tool uses the session's own tab and creates it on first use. The tools do not touch the user's other tabs unless a selector names them.

| Tool | Use |
| --- | --- |
| `chrome_launch` | Connect, or start the managed profile. `port`, `url`, `userDataDir`, `headless`. `useDefaultProfile` returns an error because Chrome blocks remote debugging of the default profile. |
| `chrome_tab` | `action`: `list`, `new`, `activate`, `close`, `version`. |
| `chrome_snapshot` | Title, URL, visible elements with stable uids (`data-pi-uid`), and page text. `mode`: `auto`, `interactive`, `forms`, `pageMap`, `text`, `changes`, `full`. Also `query`, `maxElements`, `maxTextChars`, `containingText`, `roleFilter`, `nearUid`. |
| `chrome_find` | `chrome_snapshot` with a required `query`. |
| `chrome_navigate` | `url`, `waitUntilLoad`, `timeoutMs`, `initScript`. |
| `chrome_evaluate` | `expression`, `awaitPromise`. |
| `chrome_wait_for` | `kind` (`selector` or `expression`), `value`, `timeoutMs`, `intervalMs`. |
| `chrome_click` | `uid`, `selector`, or `x`/`y`. Uses CDP mouse input. With `domFallback` (default true), a DOM `click()` runs when mouse input fails. |
| `chrome_type` | `text` at the end of the field. `replace`, `perCharacter`, `pressEnter`. |
| `chrome_fill` | Replaces the field value. `submit` presses Enter. |
| `chrome_key` | `key` and `modifiers` (`shiftKey`, `ctrlKey`, `altKey`, `metaKey`). |
| `chrome_hover` | Moves the mouse to a uid, selector, or point. |
| `chrome_scroll` | Mouse wheel events: `deltaY`, `deltaX`, `steps`. |
| `chrome_upload_file` | `paths` for an `<input type=file>` through `DOM.setFileInputFiles`. |
| `chrome_screenshot` | `path`, `format` (`png` or `jpeg`), `quality`, `fullPage`, `retentionDays`. |
| `chrome_cdp` | One raw CDP method with `params` and `timeoutMs`. `Browser.*` and `Target.*` run on the browser connection. Other methods run on the tab. |
| `chrome_cdp_targets` | Lists all CDP targets. |

Input tools accept `includeSnapshot` and `maxElements` to return a new snapshot after the action.

Background mode is on by default. In background mode, new tabs open in the background, and the extension blocks `chrome_tab activate`, `Page.bringToFront`, and `Target.activateTarget`.

Default screenshots go to `<cwd>/.pi/chrome-screenshots/<epoch-ms>.<format>`. The extension deletes default-folder captures older than `retentionDays` (default 7) and keeps the newest 20. It deletes only files whose names it made. It does not prune explicit paths.

## Command

`/chrome` opens a menu. Subcommands:

- `authorize [duration]`
- `revoke`
- `doctor`: config paths, project trust, endpoint, Chrome version, authorization, background mode, session tabs.
- `background on|off|toggle|status`: applies to this session.
- `launch`: connects, or starts the managed profile.
- `config`: select the global or project scope, then select a key and type a new value. An empty value removes the key.

## Config

Global: `<agentDir>/ext/chrome.json`. Project: `<cwd>/.pi/ext/chrome.json`. Project keys override global keys one by one. The extension reads the project file only when the project is trusted, because it can start programs and open connections. Otherwise the extension ignores the file and shows one warning.

| Key | Default | Use |
| --- | --- | --- |
| `mode` | `auto` | `connect` uses a running Chrome only. `launch` and `auto` start the managed profile when no Chrome answers. |
| `host` | `127.0.0.1` | DevTools host. |
| `port` | `9222` | DevTools port. |
| `chromePath` | Found automatically | Chrome binary. `CHROME_PATH` also works. |
| `userDataDir` | `<agentDir>/ext/chrome/profile` | Managed profile folder. |
| `headless` | `false` | Start the managed Chrome with `--headless=new`. |
| `extraArgs` | `[]` | More Chrome arguments. |
| `background` | `true` | Background mode at session start. |
| `defaultAuthorizeMinutes` | `15` | Duration for `/chrome authorize` with no argument. `0` means until revoked. |
| `closeOnExit` | `true` | Close the managed Chrome at quit. |

To use your own Chrome, start it with `--remote-debugging-port=9222 --user-data-dir=<folder>` and set `mode` to `connect`.

## State

`<agentDir>/ext/chrome/`: the managed profile and `launched.json`. `launched.json` records the browser WebSocket URL of the Chrome that the extension started. At quit, the extension closes Chrome only when the connected browser has that exact URL. A different Chrome on a reused port has a different URL, so it stays open.
