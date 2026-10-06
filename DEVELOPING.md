# Developing Violet

Practical notes for building, running and editing Violet. For design rules
see `ARCHITECTURE.md`. For the task list see `TODO.md`.

## Opening Violet

Easiest, from a terminal:

```
cd ~/projects/violet && nix develop -c cargo run
```

From fuzzel: search for "Violet". The launcher is `~/.local/bin/violet`. It
runs `target/fast/violet` and builds it first if it is missing.

Do not run `./target/debug/violet` or `./target/fast/violet` directly from a
shell outside `nix develop`. Winit can't find libwayland and the app exits
with `NoWaylandLib`. The launcher sets the library path for you, so fuzzel
and `nix develop` both work.

## Build profiles

| Command                              | Output                    | Use it for           |
|--------------------------------------|---------------------------|----------------------|
| `nix develop -c cargo build`         | `target/debug/violet`     | quick compile checks |
| `nix develop -c cargo build --profile fast` | `target/fast/violet` | daily use, what the launcher runs |
| `nix develop -c cargo build --release`      | `target/release/violet`  | shipped builds (fat LTO, slow) |

`fast` is release-speed with incremental builds, so rebuilds take seconds.

For a quick type-check while editing, use `nix develop -c cargo check`.

## Rebuilding after an edit

The launcher only builds when the binary is missing. After you change code,
rebuild the fast profile yourself, then launch:

```
nix develop -c cargo build --profile fast
```

If you forget this, fuzzel will start the old binary.

## Source layout

Under `src/`:

- `main.rs`: window setup (Glow renderer, 1280x840 default), starts the app.
- `app.rs`: the egui app itself, the per-frame loop.
- `commands.rs`: named commands. Everything that touches the document goes
  through here.
- `input.rs`, `pen.rs`: mouse and Wacom tablet input.
- `display.rs`, `compositor.rs`: drawing the canvas.
- `document.rs`, `file.rs`, `io.rs`: document state, save and load.
- `brushes.rs`: brush loading and stroke logic.
- `chrome.rs`, `layout.rs`, `theme.rs`, `keymap.rs`, `settings.rs`: UI,
  theming, keys, config.
- `perf.rs`: allocation counting and timing.

Brushes live in `assets/brushes/` as `.myb` files.

## Editing rules (from ARCHITECTURE.md and TODO.md)

- Anything that changes the document goes through a named command in
  `commands.rs`, never a direct call from UI code.
- UI elements draw into a rect they are handed. They don't store their own
  position.
- Colors come from theme tokens. Don't hardcode a color.
- Config is data, never executed.

## Known open items

See `TODO.md`. The audit list there (pen stuck-down, use-after-free at exit,
the export `.expect` that can panic) is not fixed yet. Be careful with pen
input and export until those are done.

## Working tree

The repo has uncommitted changes from earlier sessions. Check `git status`
before editing and commit in small steps so each change can be undone.
