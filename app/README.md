# Budd office

The Tauri 2 desktop app: Budd's office as a drawer on the right edge of the main screen, and the card that stands beside a desk.
This is milestone U0, the shell: the two windows, their platform behaviour, the click-through loop, the slide, and an empty office with two cubicles.
The desks, the cards' content and the connection to the core come with the later milestones.

## Run

```sh
cd app
npm install
npm run tauri dev
```

`npm run tauri dev` starts Vite on port 1420 and runs the app against it.
For a one-off binary without a dev server, `npx tauri build --debug --no-bundle` leaves `target/debug/budd-app` at the workspace root, built from `dist/`.

The app is a workspace member (`app/src-tauri`), so `cargo check`, `cargo clippy -p budd-app --all-targets -- -D warnings` and `cargo test -p budd-app` work from the repository root.
`npm run build` type-checks the pages with `tsc` and bundles them.

Versions: Tauri 2.12.5 with `tauri-build` 2.7.4, plugins `single-instance` 2.5.5, `opener` 2.7.2, `log` 2.10.1; `objc2` 0.6.5 with `objc2-app-kit` and `objc2-foundation` 0.3.2 and `block2` 0.6.2 on macOS; `windows` 0.62 on Windows; `@tauri-apps/api` and `cli` 2.12.4, Vite 7.3.7, TypeScript 5.9.3.
The office text uses Nunito 700 (SIL Open Font License, `public/fonts/OFL.txt`), subset to Latin as a woff2.

## The window model

Two windows, both borderless, transparent, always on top, on every workspace, off the taskbar, created hidden.

| Window | Size | Keyboard | Mouse |
| --- | --- | --- | --- |
| `office` | 338 pt wide (12 pt shadow margin, 26 pt tab, 300 pt drawer), height from the layout for the number of desks, against the right edge of the primary monitor's work area, vertically centred | never | only the tab, and the drawer while it is open |
| `card` | 480 by 300 pt for now, left of the open drawer, level with a desk or with the drawer's top, kept inside the work area | only while a card is interactive, and only when a human asked | while interactive |

The drawer slides in CSS inside the fixed office window: the tab and the drawer sit in one container that is `translateX(300px)` when shut, so only the tab shows at the screen's edge, and `translateX(0)` when open, 0.25 s ease-out.
The window has no system shadow; two rounded elements under the canvas cast CSS shadows for the tab and the drawer.

Click-through is a Rust loop (`shell.rs`), 30 Hz, 60 Hz while the cursor is within 40 pt of the hit region.
It reads the cursor natively (`NSEvent.mouseLocation` on macOS, `GetCursorPos` on Windows; Tauri's own getter is a round trip to the main thread, which cost about 4% of a core at this rate) and calls `set_ignore_cursor_events` on the office only when the answer changes.
The hit region is the tab when shut and tab plus drawer when open.
While the cursor is over the office window the page gets a `pointer` event in drawer coordinates (the tab spans x -26 to 0) each time the cursor moves, and one `null` when it leaves.
The office is placed again every 2 s, so a change of monitor or work area is followed.

### macOS

Tauri's windows are `NSWindow`s of tao's class.
Before the first show, both are converted in place (`object_setClass`) to `NSPanel` subclasses defined with objc2's `define_class!` (`native/macos.rs`): `BuddOfficePanel`, whose `canBecomeKeyWindow` is always false, and `BuddCardPanel`, whose answer is a static flag that is true only while the card is interactive; `canBecomeMainWindow` is false for both.
The subclasses add no instance variables, which is what makes the class swap on an existing object sound.
After the swap each panel gets style mask Borderless | NonactivatingPanel, the private `_setPreventsActivation:` sync that the class swap skipped, level Floating, collection behaviour CanJoinAllSpaces | FullScreenAuxiliary | Stationary | IgnoresCycle, `hidesOnDeactivate` false, `movable` false.
The app's activation policy is Accessory (set in code, and `LSUIElement` in `Info.plist` for the bundle), and Tauri's default menu stays so Cmd-C and Cmd-V work in the card.

`tauri-nspanel` 2.1.1 was evaluated first: it builds against Tauri 2.12 and objc2 0.6, but its `canBecomeKeyWindow` is a value baked into a macro, dynamic only by the accident of the macro inlining the expression, and it carries mouse tracking and delegate proxying this app does not need.
The swizzle here is about 150 lines modelled on it, with its `_setPreventsActivation:` detail kept.

Showing goes through objc2 too: the office with `orderFrontRegardless`, the card with `makeKeyAndOrderFront` followed by making the `WKWebView` the window's first responder when the card takes the keyboard (key status alone sends keys to the window, not to the page), else `orderFrontRegardless`.
The app never activates, so hiding the card gives the keyboard back to whatever had it.

One thing to know: Tauri's default menu binds Cmd-W to Close Window, which AppKit resolves before the window sees the key.
In the card that would close the card window itself.
Milestone U3 prunes the menu to the Edit items.

### Windows (compiled, not yet run)

Both windows get `WS_EX_TOOLWINDOW`; the office also gets `WS_EX_NOACTIVATE` and a `SetWindowSubclass` procedure answering `WM_MOUSEACTIVATE` with `MA_NOACTIVATE` (`native/win.rs`).
`scripts/check-windows.sh` type-checks this from a Mac: `cargo check --target x86_64-pc-windows-msvc -p budd-app`, with `llvm-rc` when LLVM is installed and otherwise `scripts/rc-stub.sh`, a stand-in resource compiler that satisfies tauri-build's resource step for a check build and must never be used for a real build.

## Pages

`index.html` is the office: one canvas drawn by `src/paint.ts` with the same numbers as the Swift app's `OfficeLayout` (`src/layout.ts`, `src-tauri/src/layout.rs`).
It draws the wall gradient, the header with "Office" and a wall clock from the Mac's time, two empty cubicles with the window frame and sill at the design positions, the footer rule, and the tab with its chevron (pointing out when shut, in when open), the little desk and the crew count.
A tick at 12 a second shut and 15 open repaints only when something visible changed, so an idle office costs no frames; the painters that animate will mark every tick dirty.
Clicking the tab calls the `office_toggle` command; Rust flips the hit region and emits the `office` state, and the page slides.

`card.html` is the card: a frame that follows `prefers-color-scheme`, a textarea and a Close button.
It focuses the textarea on the `focus_compose` event, never on its own; Esc or Close calls `card_hide`.

## Commands and events

| Command | What it does |
| --- | --- |
| `office_toggle` | Opens or shuts the drawer; returns the state |
| `office_state` | The state for a page that just loaded |
| `card_show { seat, interactive, focus }` | Places the card beside desk `seat` (null: level with the drawer's top), opens the office if shut, shows it with or without the keyboard |
| `card_hide` | Hides the card |
| `debug(cmd)` | The debug table below, dev builds only |
| `snapshot_save { dir, name, png }` | Writes a page's PNG where `snapshot:<dir>` asked, dev builds only |

Events: `office { open, desks }` to the office page, `pointer { x, y } | null` to the office page, `focus_compose` to the card page, `snapshot <dir>` to the office page.

## Debug channel

With the app running, a second launch relays its arguments through the single-instance plugin and exits, so from a terminal:

```sh
target/debug/budd-app --debug office:open
```

| Command | Effect |
| --- | --- |
| `office:open`, `office:close` | Slide the drawer |
| `card:show` | The card beside desk 1, interactive, with the keyboard |
| `card:peek` | The card beside desk 0, click-through, without the keyboard |
| `card:hide` | Hide the card |
| `info` | Log each window's native state: class, style mask, level, collection behaviour, key state |
| `snapshot:<dir>` | Save the office canvas as `<dir>/office.png` |
| `quit` | Quit |

Dev builds also log every key press the process receives and every key the card page sees, at debug level, which is what the focus test reads.

## Verified on macOS

Run on this Mac (one 1920 by 1080 display at 1x) with the debug binary and scripted input, the app quit after each test.

- Hover and click on the tab with TextEdit holding the caret: TextEdit stayed the frontmost app while hovering, after the click that opened the drawer and after the one that shut it, and every character typed meanwhile landed in its document (`abcdefghi`); the drawer did open and shut on those clicks.
- Full-screen and spaces: read back through objc2, both panels report class `BuddOfficePanel` / `BuddCardPanel`, style mask `0x80` (Borderless | NonactivatingPanel), level 3 (floating), collection behaviour `0x151` (CanJoinAllSpaces | Stationary | IgnoresCycle | FullScreenAuxiliary); the office answers `canBecomeKey=false` always.
  A screenshot over a full-screen app was not possible here (`screencapture` has no Screen Recording permission).
- Card focus: `card:show` makes the card key without activating the app (TextEdit stays frontmost), the textarea is focused and receives typed characters, Esc hides the card and drops key status, and characters typed afterwards land in TextEdit again.
  Caveat: the synthetic Escape had to be posted to the app's process; Escape posted at the HID event tap was not delivered to the key panel while TextEdit was the active app, although typed letters were and although it was delivered with Chrome active.
  A check with a physical Esc key is still worth doing.
- CPU with the office shut and the pointer away: 0.7 to 0.8% of a core in the main process plus 0.1 to 0.2% in the office's WebContent helper, against the Swift app's 1.5%.
  With the pointer resting on the tab the loop runs at 60 Hz; before the pointer stream was limited to movement that cost about 4% plus 3%.
- `cargo check --target x86_64-pc-windows-msvc -p budd-app` passes through `scripts/check-windows.sh`.

## Not yet verified

- Anything on Windows beyond compiling: the ex-styles, the subclass, the click-through loop, WebView2 focus.
- The card's placement against the real desk positions, since there are no desks yet.
- Reopen (`RunEvent::Reopen`) for an accessory app, planned for U7.
