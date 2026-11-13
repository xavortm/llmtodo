# Bundled themes

- `gruvbox.json`: copied unchanged from GPUI Kit (gpui-component) `themes/gruvbox.json`
  at revision `1029e2c`. GPUI Kit is © 2024 - 2026 Longbridge, Apache License 2.0.
  The Gruvbox palette is by Pavel Pertsev, <https://github.com/morhetz/gruvbox>,
  under the MIT/X11 license.
- `macos.json`: written for llmtodo, MIT like the rest of the repo. The values
  follow Apple's published system colors for macOS and Xcode's default code
  colors. It contains no Apple artwork or fonts. The theme asks for the system
  font, which macOS supplies.

The app applies its own contrast rules over these colors at run time. See
`crates/ui/src/shell/theme.rs`.
