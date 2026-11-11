# GPUI spike

Both risks from `SPEC.md` are solved well enough to build on. `+++` sections fold in the GPUI Kit code editor and collapse in the preview. Vim keys work through key contexts and stay silent while you type. One gap remains. GPUI Kit has no public API to fold or unfold a section from code, so folding in the editor works only with the mouse for now.

The app's views and model live in `crates/ui`, and the binary in `crates/app`. Run it with `cargo run -p llmtodo-app`. Run the tests with `cargo test -p llmtodo-ui`. I did not open the app window myself. The layout and colors still need the owner's preview.

## Pinned revisions

| What | Pin |
|---|---|
| GPUI Kit | git `https://github.com/longbridge/gpui-component`, rev `1029e2c5d34d75fa82ab5f0f067771951a88d993` |
| GPUI | `gpui-pre =0.3.12` from crates.io, pulled in by GPUI Kit |
| Source of `gpui-pre 0.3.12` | zed `d2a4fb6ded7c470be25d1541281da3882495b65b`, 2026-11-08 |
| Toolchain used | rustc 1.99.1 |

GPUI Kit does not depend on GPUI through git. At `1029e2c` it pins the crates.io snapshot `gpui-pre =0.3.12`, and its lock file has the same checksum as ours. So the app depends only on `gpui-kit` and takes GPUI through it. A separate GPUI git dependency would create a second copy of GPUI that does not match.

**Mismatch to report.** The Zed clone in `~/Development/refs/zed` sits at `f0d896f`. The GPUI we compile is from `d2a4fb6`, which is 25 commits older. Between them, 24 files in `crates/gpui/src` differ. The key-context code is the same in both. I did not move the clone, because the brief said not to touch other repos. The `CLAUDE.md` rule says to check out `d2a4fb6` there. Until then, read GPUI source in `~/.cargo/registry/src/*/gpui-pre-0.3.12` instead.

The GPUI Kit clone is already at `1029e2c`. The copied skills in `.claude/skills` match it file for file.

## What works

- **Shell.** Title bar, sidebar, issue list, detail pane and status bar. The base font is 14px for Zed-like density. Issue keys and dates use the theme's monospace font. Status icons are Lucide icons from GPUI Kit's bundle.
- **Monochrome theme.** `theme.rs` maps GPUI Kit's blue tokens for selection, focus rings, links and primary buttons onto shades of the foreground, in dark and light mode. Only done issues get color, a green check, and their titles turn gray. Editor syntax highlighting keeps its colors.
- **Sidebar.** It starts hidden. `Cmd-B`, `:sidebar` or the title bar button shows it.
- **List columns.** Status, key, title, assignee and date added. An agent assignee shows as an icon, and a person shows as a name. Lucide has no brand marks, so Claude uses `sparkles` and a local model uses `cpu` for now.
- **Grouping.** `:group status|label|assignee|none` or the Group dropdown splits the list under group headers. Label groups show the label color. An issue with two labels shows in both label groups.
- **Collapsible groups.** A click on a group header, or `Enter` on a selected header, opens and closes the group. `za`, `zc` and `zo` toggle, close and open the current group, and `zM` / `zR` close and open all, like vim folds. Headers can be selected, so `j` / `k` stop on them.
- **Drag to change status.** Under "group by status", drag a task onto another group's header, or onto any task in that group, to move it to that status. A card follows the pointer, and the drop target highlights. Other groupings do not allow drags.
- **Claimed tasks.** A task an agent works on shows a colored `Locked` badge over its status column, and the row gets a faint tint. The badge grows in when the agent takes the task, and shrinks away when it releases, driven by GPUI Kit's `spring`. The task view shows which agent holds it, in which worktree and herdr pane, and the task is read-only: `E`, the Edit button, status, labels, assignee, Trash and drags are all refused with a message. Claims arrive live from the server. `:release` ends one.
- **Fonts.** Zed's defaults: IBM Plex Sans at 16 px for the UI and Lilex at 15 px for code, bundled from Zed's `assets/fonts` under the SIL Open Font License. List rows and Markdown use `text_sm`, 14 px, and code blocks in the preview match that size.
- **Title bar.** The project name on the left and its folder on the right. A click on the folder copies it. The folder is `$LLMTODO_PROJECT_DIR`, or the working folder. The sidebar toggle sits left of the filter.
- **Saved settings.** Theme, sort, grouping, closed groups, sidebar, pane sizes, open tabs and the window frame survive a restart. The app writes `ui.json` after every change, so a crash keeps them too. The window frame is saved once the window rests for half a second, and at quit. Saved tabs drop tasks the data no longer holds. It lives in `~/Library/Application Support/llmtodo` on macOS, or `$LLMTODO_CONFIG_DIR` when set. A missing or broken file falls back to defaults.
- **Labels.** Each issue shows its labels as small colored tags. Labels are the one place the monochrome theme allows color.
- **Layout.** List rows and the description use the same text size. The filter bar and the task title bar share one height, and so do the column headers and the task's label bar. A drag on the divider resizes the task view, through GPUI Kit's `h_resizable` and `resizable_panel`.
- **Stacked layout.** When the list and the task view have less than 64 rem together, the task view moves under the list. The sidebar counts against that width. Each layout keeps its own divider size. `Ctrl-A j` / `Ctrl-A k` move between the panes too.
- **Right-click menu.** A right click on a task opens a menu with Status and Labels submenus, Copy ID, Edit description and Move to Trash. Move to Trash is red. `j` / `k`, `Ctrl-N` / `Ctrl-P` and the arrow keys move in it, and `Ctrl-C` or `Esc` closes it. Every item also has a palette command.
- **Labels.** The Labels dropdown in the task view and `:label <name>` add or remove a label.
- **Copy ID.** A click on a task ID, in the list or the task view, copies it. `:copy` copies the selected task's ID.
- **Trash.** `:delete` or the menu moves a task to the Trash. `:restore` brings back the last one, so there is no confirmation dialog.
- **Edit / Preview toggle.** The task view's title bar has an Edit and Preview button pair. The current mode is a filled button, and the other is a ghost button with dimmed text. Edit opens the editor like `E`. Preview saves and returns to the preview like `Esc`.
- **Focus outline.** The focused task view draws the same outline as a focused input: a one-pixel ring border at the theme radius plus GPUI Kit's focus ring. It shows for keyboard focus only, like CSS `:focus-visible`. A click focuses the task view without the outline, and the next key brings it back.
- **Contrast.** `theme.rs` raises muted text, danger red and label colors until they reach at least 4.5:1 on every surface they sit on, in dark and light mode. A test checks it. Markdown body text is slightly softer than headings, and headings are semibold.
- **Mouse.** A click on a task opens it to the side and keeps focus in the list. A click inside the preview moves focus into the task view.
- **Panes.** `h` / `l` and `Ctrl-A h` / `Ctrl-A l` move focus between the list and the task view. `Enter` opens a task, and a second `Enter` moves into it. A thin outline marks the focused task view. The list draws its selection fainter while it does not hold focus.
- **Task view.** `j` / `k` and the arrow keys scroll the description. `Ctrl-D` / `Ctrl-U` scroll half a page, and `gg` / `G` jump to the top and bottom. `:`, `?` and `E` still work there. `Esc` returns to the list and keeps the task open. The palette gives focus back to where it was opened.
- **Demo content.** LLM-101, LLM-102 and LLM-105 have long descriptions with headings, lists, a table, code and several `+++` sections.
- **App icon.** `crates/ui/assets/icon` holds `HardHat.icns` and the 1024 px PNG master. `cargo run` sets the Dock icon at startup through AppKit, because GPUI has no icon API. `scripts/bundle-macos` builds `target/release/llmtodo.app` with the `.icns` and an `Info.plist`.
- **Sorting.** Click the Status, Key or Added header, or run `:sort status|added|key`. The same key again reverses the order, and the selected issue stays selected.
- **Feedback states.** Every clickable element shows hover, pressed and selected states. That covers list rows, palette suggestions, sort headers, sidebar items, section headers and the title bar button. Elements that are not clickable get no hover change.
- **Vim keys.** `j`/`k` and the arrow keys move. `gg` and `G` jump to the top and bottom. `Enter` opens the detail pane, and the pane follows the selection. `Esc` goes back one step. `/` focuses the filter. `:` opens a small command palette. `e` opens the editor, and `E` (Shift+E) opens `$EDITOR`. `gt` / `gT` switch the task view's tabs. `?` shows every key binding.
- **Vim in the editor.** The editor opens in normal mode. It supports `h j k l`, `w b`, `0 ^ $`, `gg G`, `i a I A`, `o O`, `x D dd`, `yy p`, `u` and `Ctrl-R`. `Esc` in insert mode returns to normal mode. `Esc` in normal mode saves and returns to the preview. `:w` and `Cmd-S` save, and `:q` saves and closes. Letters with no binding do nothing in normal mode.
- **No vim keys while typing.** In the filter, the palette and the editor's insert mode, `j` and `k` type text. The status bar shows `NORMAL` or `INSERT`, and in the editor it shows the editor's vim mode.
- **Command palette.** It runs `status <backlog|todo|in-progress|done>`, `assign <name|claude|local>`, `unassign`, `edit`, `sort ...`, `sidebar`, `w`, `q`, and `theme <dark|light>`. The first suggestion is always selected. `Ctrl-N`/`Ctrl-P` and the arrow keys move between suggestions, and `Enter` runs the selected one. With no suggestion, `Enter` runs the typed text.
- **Filter.** The first match is always selected, so `Enter` opens it. `Ctrl-N`/`Ctrl-P` and the arrow keys move through matches while you type.
- **Ctrl-C.** It cancels like `Esc` in the palette, the filter and editor insert mode, and it closes the `?` help. In editor normal mode and in the list it does nothing, as in nvim.
- **Help overlay.** `?` lists every key in four columns. No row wraps. The columns wrap to new rows when the window is narrow.
- **Editor folding.** The editor shows raw Markdown with tree-sitter highlighting. Each `+++ Title` line gets a fold chevron in the gutter. A click hides the section body and keeps the `+++ Title` and `+++` lines visible. Folds update as you type.
- **Tabs from `+++` sections.** The task view shows a Description tab with the text outside any section, then one tab per `+++` section, named by its title. Agent notes and the work log get their own tabs this way. `gt` / `gT` switch tabs from the list or the task view, a click works too, and `:tab <name|number>` jumps to one. Each task remembers its open tab. A task with only one tab shows no tab bar. The editor still shows the raw Markdown and folds the sections.
- **Tests.** 40 tests pass. Eleven are unit tests for the `+++` parser and the editor's vim commands. Twenty-six are `#[gpui_kit::test]` UI tests that send real key events through a headless window. They cover vim navigation, no vim keys while typing, the palette, the editor round trip, fold ranges, vim editing, the `?` overlay, the filter, sorting, grouping, `Ctrl-C`, the sidebar, pane focus, task-view scrolling, opening a task by click, copy ID, labels, the Trash, the stacked layout, the tabs, the Edit / Preview toggle, keyboard-only focus outlines, text contrast, collapsible groups, drag between status groups, saved settings and read-only claimed tasks.

## Live data (LLM-5)

The app reads and writes through `crates/client`. `backend.rs` holds the bridge.

- **A tokio thread, not GPUI's executor.** reqwest needs a tokio reactor, and GPUI's executor is not one. So the app runs one tokio runtime on a thread named `llmtodo-api`. The app sends requests over a tokio channel. The thread sends events back over a `futures` channel, and a GPUI task awaits it on the main thread. Nothing else in the app knows about tokio.
- **Finding the server.** The same rules as the CLI: `LLMTODO_DATA_DIR`, else `~/Library/Application Support/llmtodo`, then `server.json`. When nothing answers, it starts `llmtodo server run`, from next to the app binary or from `PATH`. The app never stops an older server; the CLI does upgrades.
- **Live updates.** One WebSocket on `/v1/ws`. Events that arrive within 40 ms are merged, and each touched issue is fetched once. Status, label and team events load everything again. After a reconnect, the thread reads `/v1/activity?after_seq=<last>`, or reloads when the server says the gap is too old. `Running::stop` now also closes open WebSockets, so clients notice a restart.
- **Writes.** The app changes its own copy at once, so every action shows a reaction, and sends the write as the owner. The server's answer replaces the copy. A failed write shows a message, and the server's copy undoes the early change.
- **Description saves** send the version the editor started from. When an agent changed the text in between, the server refuses the save. The app keeps the owner's text and opens the conflict view: the owner's text in the editor, the server's version beside it. Esc saves a hand merge on top of the server's version, `:mine` overwrites it, and `:theirs` drops the owner's text. `E` writes the text to a temporary file, opens `$EDITOR` in a new herdr pane or Terminal window, and saves on each write (LLM-7).
- **Statuses and labels** come from the server. The app shows the real status name and uses the type for the icon and the order. Label colors map to the nearest theme token, so the contrast check covers them; grays map to muted text.
- **Claims** by agents lock the task and show the colored `Locked` badge, which lies over the status column so rows stay aligned. The owner's own claims do not lock. `:release` ends an agent's claim, because claims never time out.
- **Tests.** UI tests use `demo.rs`. `live_tests.rs` runs the real server in-process and drives the API thread: snapshot, writes, live claims, a stale save and catch-up after a restart.

## APIs to use

### `+++` folding in the editor

- `EditorState::new(window, cx).language("markdown")` gives a code editor with folding on by default.
- `EditorState::set_highlighter_factory(factory, cx)` installs our own highlighter. Call it before the editor first renders. GPUI Kit installs its own only when none is set.
- Implement `gpui_kit::base::input::InputHighlighter`. Our version, `fold_highlighter.rs`, wraps `gpui_kit::component::highlighter::SyntaxHighlighter::new("markdown")` for colors. It returns the `+++` ranges from `fold_ranges` as `FoldRange::new(start_line, end_line)`.
- After each edit, `update` calls `cx.defer_in(...)` and then `EditorState::apply_highlighter_fold_candidates(ranges, cx)`. The editor's incremental fold update only reparses the edited lines. One new `+++` line can re-pair every section below it, so we replace all ranges instead.
- `EditorState::display_map().is_fold_candidate(line)` and `.is_folded_at(line)` read fold state. The tests use them.

### Tabs in the preview

- `sections::tabs(text)` returns the Description tab, then one tab per `+++` section. Each tab's body renders with `TextView::markdown(id, text)`.
- `gpui_kit::component::tab::TabBar::new(id).underline().selected_index(ix).on_click(...)` draws the tabs.
- GPUI Kit also has `TextView::markdown_block_parser` and `markdown_block_renderer` for custom Markdown blocks. I did not use them. The Markdown parser sees `+++ Title` as a plain paragraph and the body as separate sibling nodes, so grouping them back into one section would be awkward.

### Vim key contexts

- The root element builds its context with `KeyContext::default()`, `add("Workspace")` and `set("mode", "normal" | "insert")`. The mode is `insert` when the filter, the palette or the editor holds focus, which `focus_handle(cx).contains_focused(window, cx)` reports.
- Bind letters with the predicate `"Workspace && mode == normal"`. Bind `escape` with `"Workspace"`, so it works in both modes.
- Put the mode on the Workspace context itself. GPUI tries a binding against each ancestor of the focused element, and it checks `mode == normal` on the innermost context of that ancestor chain. A binding for `Workspace` would fire inside a child `Input` if the mode lived only on the input.
- Escape works because GPUI Kit's `Input` and `Editor` call `cx.propagate()` when they have nothing to dismiss. GPUI then runs the next matching binding, which is ours.
- `"g g"` works as a two-key sequence. `":"` and `"shift-g"` match as written.

### Vim inside the editor

GPUI Kit's editor has no vim mode, so `vim_editor.rs` adds a small one.

- The workspace wraps the editor in an element with the context `VimEditor vim=normal|insert`. Normal-mode bindings use `"VimEditor && vim == normal"`.
- Motions bind straight to the editor's own actions in `gpui_kit::component::input`, such as `MoveLeft`, `MoveToNextWord`, `MoveToStart` and `Undo`.
- Other commands are planned as plain text edits and applied with `EditorState::set_selected_range` and `replace`. These are `dd`, `x`, `D`, `o`, `O`, `p`, `yy`, `0`, `^`, `$` and the insert commands.
- In normal mode, a `capture_key_down` listener on the wrapper stops keys that no binding handled. GPUI types a character only when its key-down event propagates, so unbound letters do not type.
- The editor's own edit actions also run in normal mode, because the editor binds keys such as `backspace` and `enter` in a deeper context. A `capture_action` on the wrapper stops `Backspace`, `Delete`, `Enter`, `Paste`, `Cut`, indent and delete-word actions in normal mode.
- Read-only mode is no help here. It also turns off the editor's `Undo` handler, so `u` would stop working.

## Workarounds and failures

- **Theme edits need `Theme::sync_base`.** Changes made through `Theme::global_mut` reach GPUI Kit's base layer only after `Theme::sync_base(cx)`. `theme.rs` calls it. Earlier builds skipped it, so some base-layer parts kept the stock tokens.
- **Heading colors need a custom text style.** The component `TextViewStyle` has no heading color. `theme.rs` builds the base `TextViewStyle` itself and installs it with `TextViewDefaults`, keeping the installed code highlighter. `Theme::sync_base` replaces it, so `set_mode` installs it again afterwards.
- **Bound keys skip key-down listeners.** GPUI runs a matched binding and stops, so `capture_key_down` never sees `j` in the task view. The keyboard-focus flag uses `cx.observe_keystrokes`, which sees every key.

- **No programmatic fold or unfold.** Only the gutter click folds a section. `display_map` is read-only from outside GPUI Kit, and its `set_folded` and `toggle_fold` are not exposed on `EditorState`. `unfold_at` exists, but it only opens folds. So `E` cannot open the editor with sections folded, and there is no keyboard fold such as `za` in the editor. Fix: a small upstream PR that adds `EditorState::set_folded(line, bool)` and `toggle_fold(line)`, or a fork until it lands.
- **Fold icons cannot be tested.** GPUI Kit paints the gutter chevrons in a custom element without test IDs. The tests check the fold ranges, not the click.
- **Full reparse on each edit.** The highlighter passes no edit to tree-sitter, to avoid a direct `tree-sitter` dependency. This is fine for issue descriptions, but slow for large docs. The real app should pass the edit through.
- **Input events in tests.** Input subscribers run only when the update that emitted the event ends. A test must end its `update_window` block before it asserts on the result of an Enter in an input.
- **All icons are bundled.** `AllAssets` embeds every Lucide icon. The real app should list its icons with `gpui_kit::assets::icon_assets!`.

## Open risks


- **Right-click menu and resizing are not tested.** The tests drive the same actions through the palette instead. The owner's preview checks the menu and the drag.
- **Pressed and hover states are not tested.** GPUI's test helpers cannot read element styles, so only the owner's preview checks them. The pressed shade is the foreground at 12% opacity, set in code, not as a theme token.
- **`Ctrl-A` may clash with herdr.** The app uses `Ctrl-A` as its pane prefix, like tmux. If herdr uses the same prefix, the terminal multiplexer takes the key first when the app runs inside it. The app runs in its own window, so this only matters if the owner sets up the same keys globally.
- **Dock icon at runtime uses `unsafe` AppKit calls.** It is small and only runs on macOS, but a bundled `.app` is the clean path. The real release should ship the bundle.
- **Agent icons are stand-ins.** The real Claude mark needs a decision about brand use and its own SVG.

- **Vim layer is small.** It has no counts such as `3j`, no visual mode, no `.` repeat, no `cw` or other operator-motion pairs, no search with `/`, and no system clipboard for `p`. The cursor sits between characters, not on one. A fuller vim needs a real operator and motion model. Zed's `vim` crate shows one, but it is GPL, so we can read it but not copy it.

- **Visual check pending.** The tests render full frames, but no one has looked at the window yet. Spacing, borders and the title bar need the owner's preview.
- **`:` on real keyboards.** `:` arrives as `shift-;` on most layouts. The test passes, and Zed binds `:` the same way. A non-US layout still needs a manual check.
- **Mode depends on focus.** Any new text field must be added to `vim_mode`, or vim keys will fire inside it. The real app should derive the mode from one place, for example a list of input focus handles or a check for `Input` in the focused context stack.
- **GPUI moves fast.** GPUI Kit pins an exact `gpui-pre` snapshot, so upgrades mean moving GPUI Kit's rev and taking whatever GPUI it brings. Re-run these tests after each upgrade.
- **Section rules are mine.** Markers must start at column 0, sections do not nest, and a section with no closing line runs to the end. They need to match what the Linear import produces.
- **Spike shape, partly done.** LLM-45 split the window into entities. `store.rs` holds the issues by ID. `list.rs`, `task_view.rs` and `palette.rs` draw the list, the task view and the palette, and `workspace.rs` owns every write. Rows, menus, drags and the open task hold issue IDs, never list positions. Clicks in a child call the workspace through a weak handle, so no entity is borrowed twice. GPUI Kit's `Sidebar`, `List` and `DockArea` are still not used.
