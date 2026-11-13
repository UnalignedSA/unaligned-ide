# Screenshots: how an agent looks at the IDE

One command starts a private instance of the app, puts it in a named state, writes a PNG and stops the instance. Open the PNG with your image-reading tool and check that it shows what you expect before you report anything about the UI.

```sh
script/unaligned/shot editor                 # a standard scene      -> .work/shots/editor.png
script/unaligned/shot my-check --open src/store.rs:44 --actions 'git_panel::ToggleFocus'   # a one-off
script/unaligned/shot my-find --from find --size 1000x640   # a standard scene, modified, under another name
script/unaligned/shot-all                    # every standard scene  -> .work/shots/*.png, index.txt
script/unaligned/shot --list                 # the scenes
script/unaligned/shot --list-actions search  # action names containing "search"
```

The last line printed on standard output is the path of the PNG. The exit status is not 0 when anything went wrong, and the reason is printed: the app crashed (the panic and its first frames in `crates/` are shown), a step named an unknown action, the window never appeared, the picture is blank, the screen is locked.

A shot takes 5 to 8 seconds. Pictures are 1440x900 by default, one pixel per point.

After the 2026-11-13 rename six scenes were captured again with the normal backend (`command-palette`, `chat-reply`, `settings`, `terminal`, `keymap`, `full-layout`); the whole set (`shot-all`) and the offscreen build were not. The rename changed the binary `shot` starts (`target/debug/unaligned-ide`), the offscreen binary (`.work/bin/ide-offscreen`), the action names the scenes use (`app::…`) and the variables of the hook (`IDE_WINDOW_SIZE`, `IDE_WINDOW_POSITION`). The durations and the behaviour described on this page were observed on 2026-11-12, before it.

## Before the first shot

- Build the app: `cargo build -p ide --features gpui_platform/runtime_shaders`. `shot` photographs `target/debug/unaligned-ide`: rebuild after a Rust change. Changes under `assets/` (themes, default settings, keymaps, icons) need no rebuild, a debug build reads them at start.
- For shots that do not need the screen (see Backends): `script/unaligned/build-offscreen`, again after every build of the app. With a locked screen this is the only backend that works, and `shot` refuses an offscreen binary older than `target/debug/unaligned-ide`.

## Taking a shot of your own area

Use a name of your own, so the standard pictures stay what the list says. Either describe the state yourself, or start from a standard scene with `--from` and change what you need:

```sh
# The find widget in a TypeScript file, in the One Light theme, in a smaller window
script/unaligned/shot find-ts --open web/app.ts:20 \
  --actions 'buffer_search::Deploy; type: task' \
  --settings '{"theme": {"mode": "light", "light": "One Light", "dark": "One Dark"}}' \
  --size 1100x700

# The standard git panel scene with the panel docked on the right
script/unaligned/shot git-right --from git-panel --settings '{"git_panel": {"dock": "right"}}'
```

Options (`script/unaligned/shot --help`):

| Option | Meaning |
| --- | --- |
| `<name>` | A scene of `scenes.json` (the options then override it) or any new name. Output: `.work/shots/<name>.png`. |
| `--from SCENE` | Start from that scene's entry, under the new name. |
| `--actions STEPS` | What to do once the window is ready. See Steps. **Replaces the scene's steps**, it does not add to them: with `--from` repeat the steps you still need. |
| `--open PATH[:LINE[:COL]]` | File to open, relative to the project. Repeatable. Files open in parallel, so the tab order and the active tab can vary: add `pane::ActivateItem N` (from 0) when it matters. |
| `--settings JSON` or `@file` | Merged into the instance's `settings.json` before launch (theme, dock positions, fonts, anything). |
| `--keymap JSON` or `@file` | Written as the instance's `keymap.json`. |
| `--project DIR` / `none` | Another folder to open (it is copied, never modified), or no project at all. Default: the sample project. |
| `--size WxH`, `--position X,Y` | Window size and position in points. Default 1440x900 at 40,40. |
| `--settle S`, `--wait S` | Seconds to wait before the first step (default 2.5) and after the last one (default 1). Raise `--wait` for slow content. |
| `--fake-api` | Connects the instance to a local stand-in for the Unaligned API, so the AI screens are in their connected state with no real key. |
| `--fake-api-key TEXT` / `none` | With the stand-in: the key the instance starts with instead of `sk-unal-screenshot`, or `none` for no key at all (the provider still points at the stand-in: connection screen, typing a key, error states). Implies `--fake-api`. |
| `--no-capture` | Run the scene and its steps and take no picture: always the normal binary (or `--binary`), works with a locked screen, keeps `.work/run/<name>` and prints that path instead of a PNG path. For checks that read the logs, the settings or the stand-in API's log. Scene key `no_capture`. `netcheck` uses it. |
| `--backend auto\|screen\|offscreen` | See Backends. |
| `--window largest` | Capture the largest window instead of the front one. |
| `--retina` | Keep the display's pixel density (2880x1800 on a Retina display). |
| `--keep`, `--keep-failed` | Keep `.work/run/<name>` (settings, project copy, app output in `output.log`, the app's log `data/logs/unaligned-ide.log`, stand-in API log `fake-api.log`) always, or when the shot fails. On a failure the end of the newest `*.log` found there is printed, whatever the log file is called. |
| `--env KEY=VALUE`, `--binary PATH`, `--out PNG`, `--timeout S` | As named. |

### Steps

`--actions` takes a JSON array, or a short form with steps separated by `;`.

| JSON | Short form | Effect |
| --- | --- | --- |
| `500` | `500` | Wait 500 ms. (Every other step is followed by 150 ms.) |
| `"pane::SplitRight"` | `pane::SplitRight` | Dispatch an action by name, to the focused element of the front window. |
| `["pane::ActivateItem", 2]` | `pane::ActivateItem 2` | Action with arguments (the same JSON as in a keymap file). |
| `{"keys": "cmd-p down enter"}` | `keys: cmd-p down enter` | Keystrokes, in keymap syntax. They go through the app's key bindings. |
| `{"type": "some text"}` | `type: some text` | Text typed into whatever has the focus (editor, search field, terminal, chat box). |
| `{"click": [300, 200]}` | `click: 300,200` | Left click at a position in the window, in points from its top-left corner. |
| `{"right_click": [130, 226]}` | `right_click: 130,226` | Right click: opens context menus. |
| `{"hover": [300, 200]}` | `hover: 300,200` | Moves the pointer there: hover styles. A tooltip appears after a delay: put a wait after it. |
| `{"press": [366, 574]}` | `press: 366,574` | Moves the pointer there, then presses the left button and **keeps it down**. |
| `{"drag_to": [160, 400]}` | `drag_to: 160,400` | Moves there with the button held, in four moves 20 ms apart, each followed by a frame. The first move that leaves the press point by more than 2px starts the drag; the next ones are seen by the drop targets drawn since. One `drag_to` is enough; several can be chained to pass through a point. |
| `{"release": [160, 400]}`, `{"release": []}` | `release: 160,400`, `release` | Button up at that point, or where the pointer is. Leave it out to picture the drag in mid-gesture. |

A drag held in mid-gesture: `--actions 'press: 366,574; drag_to: 160,400; 300'` (scene `panel-drag-zones`: the three drop zones and the chip under the pointer); add `; release; 2500` for the result of the drop. A drag shorter than 2px in total starts nothing, like a real mouse.

Keystrokes and mouse events are made inside the app (`Window::dispatch_keystroke`, `Window::dispatch_event`); nothing is sent through the operating system, so they cannot land in another application. A `keys:` step sends the key-down only: a focused button, which GPUI clicks on key-up, is not pressed by `keys: enter` (routed to W3-02). Steps go to the app's front window (its key window, or the last one opened): after `app::OpenSettings` or `app::About`, the next steps and the capture apply to that window.

To find where to click, take a first shot and read the coordinates off the picture (it is one pixel per point).

### Useful actions

Find others with `script/unaligned/shot --list-actions <text>`; argument shapes are in `target/debug/unaligned-ide --dump-all-actions`.

- Panels: `project_panel::ToggleFocus`, `git_panel::ToggleFocus`, `outline_panel::ToggleFocus`, `terminal_panel::ToggleFocus`, `debug_panel::ToggleFocus`, `agent::ToggleFocus`; `workspace::ToggleLeftDock`, `ToggleRightDock`, `ToggleBottomDock`, `CloseAllDocks`.
- Editor: `pane::SplitRight`, `pane::SplitAndMoveRight`, `pane::ActivateItem N`, `pane::CloseActiveItem`, `editor::SelectAll`, `editor::ToggleFocus`, `editor::OpenContextMenu`, `markdown::OpenPreviewToTheSide`, `git::Diff`, `diagnostics::Deploy`.
- Search and pickers: `buffer_search::Deploy`, `buffer_search::DeployReplace`, `workspace::NewSearch`, `command_palette::Toggle`, `file_finder::Toggle`, `go_to_line::Toggle`, `tab_switcher::ToggleAll`, `theme_selector::Toggle`, `icon_theme_selector::Toggle`, `language_selector::Toggle`, `projects::OpenRecent`, `git::Branch`. In a picker: `menu::SelectNext`, `menu::Confirm`, `menu::Cancel`.
- Moving a panel without a drag: focus it, then `workspace::MoveFocusedPanelToLeftDock`, `…RightDock` or `…BottomDock` (leave 2 s after it: the move is a settings write).
- Pages and windows: `app::OpenSettings`, `["app::OpenSettingsPage", {"page": "AI"}]`, `app::OpenKeymap`, `app::Extensions`, `app::About`, `app::OpenOnboarding`, `workspace::OpenComponentPreview` (type a component's name to filter the gallery to it: "section label", "panel header", "text tab strip", "input shell", "selected marker", "empty state", "Button", "CountBadge"), `dev::OpenThemePreview`.
- AI: `agent::ToggleFocus`, `agent::ManageProviders`, `agent::ToggleModelSelector`, `agent::NewThread`, `assistant::InlineAssist` (`agent::OpenSettings` only opens the chat panel in this fork).

## Standard scenes

`script/unaligned/scenes.json` is the list; `shot --list` prints it. Each entry is the long form of the options above:

```json
{
  "name": "git-panel",
  "about": "Source control panel with staged, modified, added, deleted and untracked files.",
  "open": ["src/store.rs:32"],
  "actions": ["git_panel::ToggleFocus"],
  "wait": 2.0
}
```

Keys: `name`, `about`, `open`, `actions` (JSON steps), `settings`, `keymap`, `project` (`"none"` or a folder), `size`, `position`, `settle`, `wait`, `fake_api`, `fake_api_key`, `window`, `timeout`, `env`, `no_capture` (such a scene prints a directory, not a PNG: keep it out of what `shot-all` is expected to picture). `"each_theme": true` turns one entry into one scene per theme found in `assets/themes` (`theme-one-dark`, `theme-unaligned`, ...): a new bundled theme gets its scene without editing the list. `defaults` at the top applies to every scene.

To add a scene: work it out with `shot <new-name> --actions ...`, look at the picture, then add the entry and run `script/unaligned/shot <name>` once more. Keep `about` to one sentence saying what the picture must show.

| Scene | Shows |
| --- | --- |
| `onboarding` | First launch, no project |
| `welcome` | Welcome page |
| `restricted-mode` | The "Unrecognized Project" prompt |
| `editor`, `editor-split`, `editor-narrow`, `empty-window`, `markdown-preview` | Editor with the explorer; two panes; 960x600 window; no file open; Markdown preview |
| `find`, `find-replace`, `project-search` | Find widget, with replace, project search with results |
| `command-palette`, `file-finder`, `go-to-line`, `tab-switcher`, `theme-selector`, `language-selector`, `recent-projects`, `branch-picker` | Pickers |
| `settings`, `settings-ai`, `settings-appearance`, `keymap`, `extensions`, `about` | Settings window (three pages), keymap editor, extensions, About window |
| `git-panel`, `git-diff`, `outline-panel`, `terminal`, `debug-panel`, `diagnostics` | Side and bottom panels |
| `chat-connect`, `chat-providers` | Chat panel with nothing connected and no stand-in: the connection screen; the Connections modal |
| `connect-first-run`, `connect-first-run-light`, `connect-bad-key`, `connect-and-chat`, `connect-checking`, `connect-connected`, `connections-disconnect` | The connection screen against the stand-in API with no preset key: first run (also in Unaligned Light), a refused key, the newcomer's path (type a key, Enter: the chat is ready), "Checking…" in the modal, connected with the balance, the "Disconnect?" question |
| `chat-checking-key`, `chat-key-refused-sent`, `chat-waiting`, `chat-no-models` | Chat states: the key being checked, the 401 card after a message, "Waiting for <model>…", the model list unreachable (no composer) |
| `chat-new-chat-with`, `chat-bottom`, `chat-more-menu`, `chat-history-delete` | The chat's shell: the "NEW CHAT WITH" chooser, the chat in the bottom dock, the More menu, History asking before a delete |
| `chat-ready`, `chat-providers-connected`, `chat-model-selector`, `chat-reply` | Chat panel connected to the stand-in API: empty conversation, provider picker, model list, a question with a tool call and an answer |
| `chat-key-refused`, `chat-low-balance`, `chat-server-error` | The chat when the stand-in API refuses the key (the amber card before any message), reports a balance too low (402 card with the balance and Add Funds) or fails (500: the retry card, then "did not answer") |
| `inline-assist`, `inline-assist-result`, `inline-assist-no-provider` | Inline edit box: prompt typed, proposed edit, not connected |
| `full-layout` | Explorer, editor, terminal and chat together |
| `layout-explorer-right`, `layout-terminal-left`, `layout-chat-left` | A panel moved to another dock by its action: Explorer on the right, terminal as a left column, chat on the left (the activity bar's right group is then empty) |
| `activity-bar-badge`, `activity-bar-manage`, `panel-drag-zones`, `activity-bar-drag-slot` | The activity bar: a count badge on Source Control, the Manage menu, a panel held over the drop zones, the slot of an empty group during a drag |
| `layout-menu`, `layout-ai-first`, `zen`, `layout-reset` | The title bar's Layout menu, the AI-first preset, Zen mode, Reset Layout from a scrambled layout |
| `welcome-tab`, `welcome-light` | The welcome page opened by `app::ShowWelcome` (it crashed before W2-12: this scene is the guard), and in Unaligned Light |
| `about-light`, `licenses`, `command-palette-app`, `update-available` | About in Unaligned Light, the licences page, "App:" commands in Title Case, the title bar's "Update Available" button (simulated, 1000x420) |
| `terminal-ansi` | The 8 ANSI colours normal, bright and dim, in the terminal |
| `light-full-layout`, `contrast-full-layout`, `dusk-full-layout`, `midnight-full-layout`, `midnight-git-diff`, `light-git-diff`, `light-project-search` | The main scenes in the four new family themes. In Midnight the accent is azure: any red that is not a status or a keyword is a hard-coded colour |
| `context-menu-editor`, `context-menu-explorer`, `context-menu-tab` | Context menus |
| `corners-round`, `corners-forced-square` | The editor context menu in One Dark (rounded corners), and in One Dark with the setting `"corners": "square"` |
| `editor-selection` | Everything selected in a Rust file: text and syntax colours on the selection |
| `component-preview` | Every core UI component (developer page) |
| `button-primary`, `button-primary-one-dark`, `count-badge` | Gallery entries: the primary button's states in Unaligned and in One Dark, count badges. The two button scenes click a row of the filtered gallery list at fixed coordinates: look at the picture, and move the click if a new component has "button" in its name |
| `preview-section-label`, `preview-panel-header`, `preview-text-tab-strip`, `preview-input-shell`, `preview-marker`, `preview-empty-state` | Gallery entries of the six shared primitives, one per picture (1100x960) |
| `theme-<name>` | The `editor` scene in each bundled theme |

## The sample project

`script/unaligned/make-sample` generates `.work/sample/skiff` on first use (and again when the script changes): a small fictional task tracker with Rust, TypeScript, Python, Markdown, JSON and TOML files (code comments in `src/store.rs` and `web/app.ts`, and `scripts/ansi.sh`, which prints the ANSI colour table for `terminal-ansi`), in its own git repository with two commits and uncommitted changes of every kind: modified (`README.md`, `src/store.rs`), staged (`config/settings.json`), added (`web/theme.ts`), deleted (`scripts/legacy_import.py`), untracked (`notes/todo.md`). Each shot works on its own copy, so a scene may edit or delete files freely.

There is no file with diagnostics: they come from language servers, and the harness turns language servers off (see What every instance gets).

## The stand-in API

`--fake-api` (scenes: `"fake_api": true`) starts `script/unaligned/fake-api` on a free local port and points the instance's Unaligned provider at it, with the placeholder key `sk-unal-screenshot`. `--fake-api-key TEXT` (scenes: `"fake_api_key": "TEXT"`) presets another key, and `none` presets no key: the provider still talks to the stand-in, so a scene can show the connection screen and then type a key into it. It serves a model list (nine models with prices, one `down`, one `degraded`), `GET /v1/balance`, and canned streamed answers: in the chat, a `read_file` tool call followed by a Markdown answer with a code block; for inline assist, a `rewrite_section` edit; a title when the app asks for one. Nothing leaves the machine.

The text of the key chooses the state (the script's docstring is the reference):

| Key | Balance | Chat |
| --- | --- | --- |
| `sk-unal-screenshot` (default), any other text | 200, $20.50 | 200, the canned answer |
| contains `bad` | 401 | 401 |
| contains `low` | 200, $0.04 | 402 "Balance too low for this request: $0.04 available." |
| contains `err` | 200, $20.50 | 500 |
| contains `slow` | as above, after 3 s | as above, after 3 s (combine: `sk-unal-slow-bad`) |
| none | 401 | 401 |
| contains `rate` (since W3-00) | 200, $20.50 | 429 with `Retry-After: 1` |
| contains `gone` | 200, $20.50 | 404 (the model was withdrawn) |
| contains `busy` | 200, $20.50 | 503 with `Retry-After: 1` |
| contains `drop` | 200, $20.50 | the connection is closed mid-answer |
| contains `policy` | 200, $20.50 | 400 "Your request was flagged by the content policy.": the card with Edit Prompt and Retry |
| contains `filter` | 200, $20.50 | 200, an empty answer that ends with `finish_reason: "content_filter"`: the refusal card, the prompt back in the message box |
| contains `fail` | 200, $20.50 | 500 with `Retry-After: 1` (the retries end in seconds: the final card can be pictured; `err` sends no `Retry-After`) |
| contains `nomodels` | 200, $20.50 | 200; the model list answers 500 |
| contains `slowmodels` | 200, $20.50 | 200; the model list answers after 3 s |

For other content (a longer answer, more models) edit the constants at the top of the script in a scratch copy; the script itself belongs to the provider tasks.

## Backends

- `screen`: `screencapture -l <window id>` on the instance's window. The picture includes the macOS title bar buttons and the rounded window corners. Needs an unlocked screen and a window that is not fully covered (the app stops drawing a hidden window).
- `offscreen`: the app renders its own window to the PNG (`Window::render_to_image`, compiled in by the `visual-tests` feature). No screen capture, no permission, works with the screen locked or asleep and with the window covered. The picture is the app's own drawing: no macOS title bar buttons, square corners. Needs `.work/bin/ide-offscreen`, built by `script/unaligned/build-offscreen` (two builds of the `ide` crate; about 20 seconds when the rest is built, but every crate changed since the last run is compiled twice). `shot` refuses an offscreen binary older than `target/debug/unaligned-ide` instead of photographing old code. Two things to know about it: the offscreen binary has test support compiled in (the `visual-tests` feature turns on `project/test-support` and others), so it **never starts a language server** ("language server binary lookup … is disabled in tests"): a scene that needs one cannot use this backend. And while `build-offscreen` runs, `target/debug/unaligned-ide` itself is that test-support build for a few minutes (it is built there, copied, then the normal one is rebuilt): do not start the app, measure it or audit its network during that window.
- `auto` (default): `screen`, or `offscreen` when the screen is locked.

For unattended work (a night), run `script/unaligned/build-offscreen` after each build and use the default: shots keep working when the screen locks. Pixel values of the two backends differ by 1 or 2 levels (the screen capture goes through the display's colour profile); do not compare a `screen` picture with an `offscreen` one pixel by pixel.

## What every instance gets

- Its own data directory, `.work/run/<name>/data` (`--user-data-dir`), created empty for each shot and deleted afterwards. Since wave 1 that option alone isolates the app (data, `data/config`, `data/state`, `data/cache`, `data/logs/unaligned-ide.log`). Its `HOME` is also set to `.work/run/<name>/home`, which stays empty: a second barrier, so that neither the app nor a child process (shell, git, npm) reads or writes the owner's configuration, and nothing can reach a real Zed installation.
- These settings, under the scene's own (override them with `--settings` when they are the subject): `enable_language_server: false` and `auto_install_extensions.html: false` (no downloads), `session.trust_all_worktrees: true` (no "Unrecognized Project" prompt), `cursor_blink: false`, `agent.notify_when_agent_waiting: "never"` (that notification is a window of its own and would be photographed instead of the main one), and a plain terminal shell (`/bin/zsh -f` with the prompt `skiff % `, so no picture carries the owner's user or machine name).
- The pointer moved out of the window before the first step (the app starts with the real pointer position, which would light up a row at random).
- An environment without any variable ending in `API_KEY`, `_TOKEN`, `SECRET` or `PASSWORD`: a real key can never show in a picture.
- The debug-only hook: `UNALIGNED_DEV_ACTIONS` (the steps, JSON) and `UNALIGNED_DEV_DONE_FILE`, read by `run_unaligned_dev_actions` in `crates/ide/src/app.rs`. Window size comes from the existing variables `IDE_WINDOW_SIZE` and `IDE_WINDOW_POSITION`. The hook is compiled only with `debug_assertions` and does nothing unless the variable is set.

## Limits

- Native macOS UI is outside the window and cannot be captured: the menu bar at the top of the screen (the application menu), file open/save dialogs, and confirmation prompts, which are native sheets unless the scene sets `"use_system_prompts": false`. The in-window application menu exists only on Linux and Windows: it cannot be photographed on this Mac.
- Drag and drop is scripted since wave 2 with `press`, `drag_to` and `release` (see Steps; scenes `panel-drag-zones`, `activity-bar-drag-slot`). The pictures show the gesture; the behaviour is proven by GPUI tests that send mouse events (`cargo test -p workspace panel_drag`, `activity_bar`). Never tried: a `drag_to` with the screen unlocked (frames from the display instead of the offscreen build's own draws).
- Popovers with the offscreen backend: `PopoverMenu` gives its menu the focus two display frames after it opens, and the offscreen binary gets no display frames, so `keys:` and `type:` right after a step that opens a popover reach nothing. Click inside the popover first (scene `chat-history-delete` does).
- Anything that needs a language server (diagnostics, hover documentation, completions from the server, inlay hints) or a real model answer. If you turn language servers on for a one-off (`--settings '{"enable_language_server": true}'`, screen backend only): every instance now runs with `RUSTUP_AUTO_INSTALL=0` (a scene's `env` or `--env` can override it), so `rustup` no longer downloads the repository's pinned toolchain into the instance's empty home.
- Linux and Windows rendering. This is the macOS build; code under `cfg(target_os = "linux" | "windows")` is not what you see.
- A window opened by a step (settings, a second workspace window) starts with the real pointer position: a row under it may show as hovered.
- With the screen locked the app is not the active application, and a few actions that look for "the active window" behave differently: `app::OpenKeymap` opens the keymap editor in a new empty window instead of a tab of the project window. The page itself is the same.
- The instance becomes the front application for the few seconds it lives (the app activates itself at start): if the owner is typing at that moment, a few keystrokes can land in the instance (a throwaway copy of the sample).
- Shots take turns. `shot` holds a lock (`.work/run/.queue`), so agents can call it at the same time and each waits for the one before; give each shot its own name. When many agents rebuild, the shared offscreen binary is stale within a minute: keep a private copy and pass it with `--binary` (a given `--binary` skips the staleness check).
- Coordinates: the side bar is 280px wide since wave 2 (it was 240), so everything right of it sits 40px further right than in wave-1 pictures. Scenes that click at fixed points (`button-primary`, `context-menu-tab`, `layout-menu`, `activity-bar-manage`, `connections-disconnect`, `chat-history-delete`) must be looked at after any change of that layout.
- Every instance starts on an empty data directory, so the welcome page's recent projects cannot be a scene (they have to be seeded by hand).
- Animations are caught wherever they are when the picture is taken; raise `--wait`.

Learned in wave 3 (added at gate 3):
- **Another platform's title bar**: in debug builds, `--env UNALIGNED_TITLE_BAR_STYLE=windows` or `=linux` draws that platform's title bar on this Mac (Linux as client-side decorations; `--env UNALIGNED_TITLE_BAR_BUTTONS=close,minimize,maximize:` sets the button layout, GNOME form). The Windows caption glyphs are placeholder boxes here (no Segoe icon font). Release builds ignore both variables. Windows that build their own title bar (Settings, the threads sidebar) do not follow the switch. (W3-01)
- **The offscreen pointer rests over the window** (about y=372): in a picker picture the row under it is hovered, and a picker that selects on hover previews that row (`theme-selector` is drawn in the hovered theme; the Appearance page of Settings previews the tile under the pointer). End such scenes with a `hover` step on a point that is never a row or a tile (`{"hover": [700, 110]}` for the Appearance page). (W3-08, W3-09)
- **`onboarding::Finish` works with the screen locked** since W3-07 (it is registered on the workspace): `welcome-finish` needs no `--backend screen`.
- **Actions sent while the Settings window is in front do not reach it** (`app::OpenSettingsAt` then did nothing in W3-08's run): open the page you want with the first step.
- **A scene that starts an external agent** (the "Set Up" of a subscription row) downloads from GitHub and can outlast the tool's allowance of 60 s plus the declared waits: the tool then reports "the actions never finished" although the picture may still be written. Not for every gate. (W3-11)
- **Turns the stand-in cannot play** (an edit with the review bar, a terminal call with its permission prompt, a slow stream): they needed a scratch wrapper around the stand-in, which is not in the repository. A fixed port for a second stand-in: start `script/unaligned/fake-api <port>` beside the shot (`--fake-api` picks its own).
- **Idle CPU cannot be measured with the screen locked** (the normal binary draws no frames) **nor with the offscreen binary** (the hook draws about twenty frames a second during waits): measure with `target/debug/unaligned-ide` on an unlocked screen. (W3-02, W3-06)

## Rules

1. Only windows of instances started by `shot`. Never capture the whole screen, never capture or stop the owner's instance (`pgrep -f target/debug/unaligned-ide` shows it: leave it alone). `shot` stops only the process it started.
2. Always the harness's own data directory: never start the app by hand without `--user-data-dir`.
3. Never commit anything under `.work/` (it is git-ignored). The only pictures in the repository are the README's, in `.github/readme/`: copies of shots taken with `--fake-api` on the sample project.
4. No real API key in an instance that is photographed: use `--fake-api`.
5. Look at the picture. A shot that succeeded only proves a window was drawn, not that it shows what the scene name says.
