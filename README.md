# Violet

Violet is a fast desktop drawing app for artists who work with a pen tablet.
It is built in Rust with egui and is designed to keep the canvas, keyboard, and
pen close to the work.

> A drawing tool with the focus of a text editor.

![Violet splash screen](assets/splash.jpg)

## What it is

- A native Linux drawing app with Wacom tablet support in development.
- A keyboard-driven command layer for tools and document actions.
- A headless document core with sparse tiled layers and reversible edits.
- Custom brush presets stored in `assets/brushes/`.
- A native `.lily` document format with save, autosave, recovery, undo, and redo.

Violet is early-stage software. Expect rough edges, unfinished tablet behavior,
and changing file formats.

## Run it

The supported development environment is Nix:

```bash
nix develop -c cargo run
```

For the faster daily build:

```bash
nix develop -c cargo build --profile fast
```

Do not run the binary outside `nix develop`; the Wayland libraries are supplied
by the development environment.

## Development

```bash
nix develop -c cargo check
nix develop -c cargo test
```

Read the [development guide](DEVELOPING.md) for the source layout, rebuild
workflow, and known issues.

The [architecture notes](ARCHITECTURE.md) explain the product direction,
startup-time budget, command model, document representation, and tablet-input
constraints. The [UI system](docs/UI-SYSTEM.md) and [Chrome notes](docs/CHROME.md)
document the interface conventions.

## Project direction

Violet aims for strong defaults, low latency, and no required configuration.
Configuration remains text-based and optional; the interface should stay quiet
while drawing.

The project is part of the Navy Lily family of creative tools. See the [project
page](https://navylily.tv/cool-software/violet) for a short overview.

## License

The repository is public while the project is being developed. Licensing and
distribution terms will be documented here before a stable release.
