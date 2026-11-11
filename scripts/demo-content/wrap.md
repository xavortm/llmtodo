Every case that once ran past the right edge of the task view. Narrow the task view and check that nothing clips: text wraps, and only code blocks and wide tables scroll sideways inside their own box.

## A heading with a long word: Donaudampfschifffahrtselektrizitätenhauptbetriebswerkbauunterbeamtengesellschaft

A paragraph with a long URL: https://github.com/xavortm/llmtodo/blob/01fd44bd7f1fb74564ccc7c637a7a473ba0ee485/crates/app/src/task_view.rs#L824-L990 and a commit hash 01fd44bd7f1fb74564ccc7c637a7a473ba0ee48501fd44bd7f1fb74564ccc7c637a7a473ba0ee485 in the middle of a sentence.

A link with a long label: [https://example.com/a/very/long/path/that/never/breaks/on/its/own/because/it/has/no/spaces](https://example.com)

Inline code with no spaces: `llmtodo_app::task_view::TaskView::render_preview::columns_fit::COLUMN_MIN_REM::preview_width` and more text after it.

AVeryLongWordWithNoSpacesAtAllThatKeepsGoingAndGoingPastAnyReasonablePanelWidthUntilItMustBreakSomewhere

## Lists

- A list item with a long path: /Users/someone/Library/Application\ Support/llmtodo/files/e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
  - A nested item: `cargo test -p llmtodo-app --lib -- tests::wrap::the_preview_never_runs_past_the_panel_edge`
1. A numbered item: sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08

- [ ] A task item: https://example.com/issue/LLM-85/app-the-description-preview-cuts-off-on-the-right

## Quote

> A quoted URL: https://zed.dev/docs/extensions/developing-extensions#publishing-your-extension-to-the-extension-registry-and-keeping-it-current

## Code block

```rust
fn main() { let url = "https://github.com/xavortm/llmtodo/blob/main/crates/app/src/task_view.rs"; println!("{url} {url} {url}"); }
```

## Wide table

| Key | Branch | Worktree | Commit | Note |
|---|---|---|---|---|
| LLM-85 | fix/wrap-the-description-preview-at-every-width | /Users/someone/Development/llmtodo-wt/wrap | 01fd44bd7f1fb74564ccc7c637a7a473ba0ee485 | Wraps every element |
| LLM-64 | feat/tabs-as-columns-on-a-wide-task-view | /Users/someone/Development/llmtodo-wt/columns | 3b0edd4c5d6e7f8091a2b3c4d5e6f708192a3b4c | Tabs as columns |

## Image

![Placeholder](https://placehold.co/1600x200/png?text=a+wide+image+scales+down)

+++ Agent notes
A section with a long URL too: https://github.com/xavortm/llmtodo/pull/12345/files#diff-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
+++
