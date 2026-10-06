# Lilyfield TODO

Drawing app. Desktop, Rust + egui, must work with Wacom tablets.
Language decided 2026-09-20: Rust + egui.
Architecture decisions live in ARCHITECTURE.md.

## WHAT LILYFIELD IS
The Neovim of drawing apps.

- Super, super easy to **GET WORK DONE**. Not to tinker.
- Opens **instantly**.
- **Does not get in the way.**
- An absolute **TOOL**.
- **Completely modular UI.** Every single UI element can be repositioned.
- **Really good scripting.**
- **Beautiful themes**, for the UI, the background, and fonts.

- **Reproducible, like NixOS.** Not imperatively configured. Config files ARE
  the state, not instructions for reaching some state.

Not a contradiction with "not to tinker": the point is **zero required
configuration**. Move everything, script everything, theme everything, and
never be made to, and never be interrupted by the possibility.

Config (keymap, layout, theme) and scripting (commands, brushes, hooks) are
two separate layers that must never merge. Config is pure data, parsed, never
run. Scripts are Lua, allowed to be a real program. A script may set the active
theme by calling a command; it may never reach in and mutate config at runtime.

Every decision below passes through that filter or it does not go in.

## INSPO
- **Our Paint** by chengdulittlea: https://chengdulittlea.itch.io/ourpaint
  UI and everything else. Infinite canvas, GPU rendering, pigment mixing that
  simulates real color theory, node-based brush editor, custom GUI framework
  (LaGUI) with 3D elements and animation, 20-30 MB binary, Windows/Linux/Android,
  Wacom pressure via standard drivers. Deliberately has **no canvas flip**,
  as a philosophy: "This helps to train your hand and eye to form a uniform look."
- Pictogrep's own UI blueprint (`pictogrep/docs/ui.md`) for the spacing, button,
  radius and type scale, so the two apps read as siblings.

## Why Rust (the actual requirements)
What the architecture has to stay compatible with, even in the prototype:
- very low input latency
- tablet events
- GPU rendering
- image buffers
- memory layout
- potentially SIMD
- threading
- large documents
- custom rendering

## THE CANVAS IS THE SACRED OBJECT
Get this working before basically anything else:
- [ ] blank canvas
- [ ] pan
- [ ] zoom
- [ ] rotate
- [ ] draw stroke
- [ ] erase
- [ ] undo
- [ ] redo
- [ ] save
- [ ] load

## Three rules that apply from the first line of code
Each is a day's work now and a rewrite later. Not optional, not deferrable.
- [ ] Nothing calls the document directly. Everything goes through a **named
      command**.
- [ ] No UI element knows its own position. It renders into a rect it is
      **handed**.
- [ ] No UI element invents a color. It uses a **theme token**, and there is
      no gray text token to reach for.
- [ ] Config (keymap/layout/theme) is **parsed data, never executed**. Scripts
      never mutate config directly, only through commands.

## Pending
- [ ] **Push to GitHub** (asked 2026-10-05).
- [ ] **Create V1 of Lilyfield as fast as possible** (asked 2026-10-05, in session 196e3a05; projects-f1 owns the code).
- [ ] **UI must be modular and easy for the user to set up** (Tiago, 2026-10-05). **Revised the same day: the UI is edited only through config files.** No in-app layout mode for now, and no app-written layout file. The Lua config is the single source for which panels exist, where each one docks (any edge, or floating), its size, and whether it shows (status line included). The app never writes config. Plugins can't add panels for now.
- [ ] **UI audit by 1 agent** (asked 2026-10-05). Read-only; findings go to Tiago first.
- [ ] **Make the app usable today and install it on Tiago's system so he can launch it** (asked 2026-10-05). Owner: session projects-f1 (Tiago's call), ahead of the rest of the restructure.
- [x] **Audit the existing code with 3 agents** (asked 2026-10-05): correctness/safety of pen.rs, main.rs vs ARCHITECTURE.md rules, build/latency/Nix setup.
- [x] **Switch renderer from wgpu to glow** (done 2026-10-05). Cargo.toml + src/main.rs, two lines.

- [ ] **Optimization quick wins** (found 2026-10-05, audit by agent):
  - `display.rs:100,143,253` — cache `paper_rgb(doc.paper_linear())` as a field on `Display`; currently computed 2-3x per frame
  - `display.rs:104` — `.drain().collect()` allocates a `Vec<TileKey>` every drawing frame; replace with a reused `Vec` field drained in-place
  - `history.rs:189` — opacity undo calls `.keys().copied().collect()` over every tile in the layer; make it lazy (dirty the whole layer without allocating)
  - CI only: `lto = "thin"` + `codegen-units = 4` in a `[profile.release-ci]` profile

- [ ] **Heavily improve the UI layout: "it's terrible"** (Tiago, 2026-10-05, after first real test). One agent.
- [ ] **Confirm before leaving or starting a new drawing** (Tiago, 2026-10-05: "otherwise you just delete your drawings").
- [ ] **A proper vertical tool bar on the left, with icons** (Tiago, 2026-10-05). Given to the layout agent.
- [ ] **A proper config/settings screen** (Tiago, 2026-10-05). One agent. Includes the obvious options, e.g. **new file asks for dimensions**.
      **Decided 2026-10-05 (Tiago): the settings screen edits the config file.** This replaces "the app never writes config". The file in ~/.config/violet/ stays the single source of truth and stays hand-editable; if Nix makes it read-only, the screen shows the values read-only.
- [ ] **Explore UI and find good rules** (Tiago, 2026-10-05). One agent; rules go in docs/ui.md (same place as Pictogrep's blueprint).
- [ ] **A screen to see all drawings** (Tiago, 2026-10-05). One agent. ("and settings of course": the settings screen above.)
- [ ] **BUG: Shift+Plus finds no command** (found 2026-10-05 by the readability agent): egui 0.36 names the key "Plus", all letters, so `Keymap::lookup`'s "symbols ignore shift" check never fires. Test `keymap::tests::redo_needs_shift_undo_does_not` fails.
- [ ] **Make the code more readable; humans will audit it later** (asked 2026-10-05). One agent. No behavior changes.

- [x] **BUG: brush stops working after switching colors** (Tiago, 2026-10-05: "it works now"). Pen-to-egui mirroring moved into `App::pen_to_egui` so tests drive the real path; `pen_paints_after_picking_a_color_in_the_square` and `mouse_picks_a_color_in_the_square_then_paints` pass.
- [ ] **UI reference from Tiago, 2026-10-05 ("use this")**: dark panel with a hue ring around a saturation/value square, a hex field top right, a row of brush slots with icons and numbers, and labeled sliders with values on the right (Size, Min size, Flow, Opacity, Stabilizer, Tip shape). Conflict to resolve: the ring and round slot buttons are round, but the fixed rule is no rounded corners. Gray labels also conflict with the no-gray-text rule. (Tiago, 2026-10-05, first test of the installed build).

- [ ] **Fast everyday build** (asked 2026-10-05: long builds break flow). `[profile.fast]` in Cargo.toml (release minus LTO, 16 codegen units, incremental); the installed `~/.local/bin/violet` runs `target/fast/violet`. Fat LTO stays in `[profile.release]` for shipped builds only.

- [ ] **No metal/reflective/animated/particle visuals, ever** (decided 2026-10-05). GPU is allowed only for raw performance (tile upload, layer compositing speed). Never for aesthetics. No complex shader-driven brushes. The app must feel instant and invisible.

- [ ] **Milestone one, step by step** (asked 2026-10-05; "oneshot" dropped
      mid-turn for "be efficient" and "always take the best decision for long
      term"). Steps 1 to 7, each verified before the next: startup probe,
      command layer + keymap, canvas with pan/zoom/rotate, two-pass brush,
      eraser, undo/redo, save/load/export with atomic writes and autosave,
      real pen timestamps. Status line and a summoned color picker on theme
      tokens, each drawn into a rect handed to it by layout data.
- [ ] **Runs on any laptop, so it is measured from day one** (asked
      2026-10-05: "it has to be super super optimized"). A perf layer in the
      core, not bolted on: input processing, stroke generation, document
      mutation, undo/redo, rendering, egui frame time, allocations per frame,
      save/load, draw calls, GPU upload. Shown in a summoned overlay, plus a
      headless benchmark binary so a slow laptop can report real numbers.
- [ ] **One very, very high leverage UI action, done** (asked 2026-10-05,
      replacing "complete ui on lilyfield" mid-turn: "just find one very very
      high leverage action and do it").

## Build order

Steps 0 to 7 are **milestone one** (one finished drawing). Steps 8 to 12 do not
start until a drawing exists. The three rules above are not steps: they apply
from the first line of code.

- [x] **0. Pen spike. CONFIRMED WORKING 2026-09-20 on real hardware:** real
      Wacom pressure drives strokes, via `zwp_tablet_v2` bound directly on
      winit's own Wayland connection (`src/pen.rs`), bypassing OTD's virtual
      device entirely. Tilt is wired but unverified against real tilt values.
- [ ] **0 (superseded).** Pressure + tilt from a real Wacom in a winit window
      under niri. **Confirmed 2026-09-20 on real hardware, mouse-drag MVP
      build: the stylus produces no input at all, not just no pressure.**
      winit only listens to the Wayland pointer protocol, and a stylus reports
      through `zwp_tablet_v2` instead, so egui never even sees a position.
      This raises the pen spike's priority: it is not a pressure nice-to-have
      anymore, it is "the tablet doesn't work" until this is bound by hand.
      - Tiago runs **OpenTabletDriver** (0.6.7, as a Nix package). It creates
        a virtual uinput device `OpenTabletDriver Virtual Artist Tablet`
        (`/proc/bus/input/devices`, confirmed present 2026-09-20) that reports
        as an absolute pointer (`INPUT_PROP_DIRECT`, `ABS_X/Y/PRESSURE/...`),
        not through the Wayland tablet protocol. That means the current
        "nothing at all" failure could be either (a) OTD not actually
        forwarding HID reports to the virtual device (no tablet profile
        configured / area not bound to a display), or (b) niri/libinput not
        routing that virtual device to clients the way it does an XY pointer.
        Diagnose with the OTD GUI's live input view and `libinput debug-events`
        before touching any Rust code. This may not be a Lilyfield bug at all.
      - **Recommendation: bind `zwp_tablet_v2` directly, bypass OTD's virtual
        device.** `smithay-client-toolkit` (v0.21.1, confirmed available on
        crates.io 2026-09-20) has a maintained `seat::tablet` module wrapping
        exactly this protocol: real pressure and tilt straight from the
        compositor, no dependency on OTD's uinput mapping or its area/output
        binding being configured correctly. Use OTD's own source
        (github.com/OpenTabletDriver/OpenTabletDriver, C#) only as a reference
        for per-tablet-model HID report parsing if `zwp_tablet_v2` itself
        turns out to be insufficient (e.g. a device niri's compositor doesn't
        expose over it) and an evdev fallback becomes necessary, not as the
        primary path.
      - **Confirmed 2026-09-20, read-only diagnosis (evtest capability dump +
        niri's journal, no device was grabbed or reconfigured):** OTD's
        virtual device (`event19`) really is being fed to niri — libinput
        "lagging behind" warnings for it appear as recently as Sep 6, so it's
        not being ignored. It reports real `ABS_PRESSURE` (0-65535) and
        `TipActivationThreshold` is 1% in OTD's own config, so "needs heavy
        pressure" is not an OTD click-threshold problem. The device is
        `INPUT_PROP_DIRECT` + `INPUT_PROP_POINTER`, which is libinput's
        touchscreen shape, not pointer or tablet-tool shape, so it is most
        likely arriving as `wl_touch`, which winit/egui here don't handle as
        drawing input at all (hence needing a hard, sustained contact before
        anything registers, and a Wacom that looks unreliable). This does not
        change the recommendation below, it confirms it: OTD's virtual device
        is the wrong path regardless of pressure tuning.
      - **Cross-platform consequence (decided 2026-09-20):** this becomes one
        `PenSource` trait with a Wayland impl, not a Wayland-specific binding.
        See ARCHITECTURE §0b. Check whether winit already carries pressure on
        Windows and macOS before writing those backends.
- [ ] **1. Startup measurement.** Script timing exec to first frame, plus a
      startup profiler. Budget: under 150ms. A budget nobody measures is a wish.
- [ ] **2. Command layer.** Named commands, keymap, dispatch.
- [ ] **3. Canvas skeleton.** Tiles, view transform, dirty-tile upload.
- [ ] **4. Brush.** Scratch buffer, stamping, pressure to size and opacity.
- [ ] **5. Eraser and history.** Destination-out, copy-on-write undo/redo.
- [ ] **6. Save, load, export.** Zip plus PNG, atomic writes, autosave.
- [ ] **7. Real pen.** Stabilization, latency tuning.
- [ ] **8. Theme tokens, then the UI.** Status line, summoned pickers.
- [ ] **9. Layout file and layout mode.**
- [ ] **10. Lua scripting.** Commands, then brushes, strokes, hooks.
- [ ] **11. Pictogrep reference panel.**
- [ ] **12. Plugin system.** Directory + plugin.toml + permissions, on top of
      the Lua engine from step 10, not a second runtime.

Steps 2 to 6 are testable with a mouse at pressure 1.0, so step 0 being
hardware-blocked never stalls the build.

## Setup
- [x] Nix dev shell (flake.nix) with Rust + what eframe needs on Wayland
- [x] cargo project scaffolded (eframe/egui 0.36.2, image, serde)
- [ ] Switch eframe to the wgpu backend before writing render code
- [ ] Add mlua

## Pictogrep integration
- [ ] Reference panel: plain-language search against the running Pictogrep,
      thumbnails pinned beside the canvas. Discovery is **lazy**, never on the
      startup path, and a connection timeout must never touch launch.
- [ ] Open a Pictogrep board as a reference set
- [ ] Save finished work into a Pictogrep-indexed folder, so paintings become
      searchable reference
- [ ] **Blocker:** `PICTOGREP_TOKEN` is invented per launch and passed by env,
      so a separate process cannot read it, and nothing publishes the real port
      after the 8765 fallback. Fix is a 0600 session.json in Pictogrep's data
      home, which changes the PUBLIC repo. Ask first.

## Decided 2026-09-20
- [x] **Rust + egui**, over Go.
- [x] **Fixed document bounds**, not infinite. Sparse tiles kept anyway, and
      never a new-file dialog between launch and the first stroke.
- [x] **No pigment mixing for now**, RGB only. All channel math behind one
      compositor module, since a later pigment mode is a compositor rewrite.
- [x] **Brushes are data**, edited as text or Lua.
- [x] **Node-based brush editor is cut.** It is in INSPO, and it is the most
      tinker-shaped feature in Our Paint. Scripting covers the same ground
      without occupying the screen.
- [x] **Lua via mlua** for scripting. Lua orchestrates, Rust does pixels, and
      the script layer never enters an inner loop.
- [x] **Not modal.** The pen always paints, so there is no insert mode to
      leave. Single keys under the left hand, no trip to a toolbar.
- [x] **Do not touch the public pictogrep repo.** Ask again when the reference
      panel is actually being built.
- [x] **Plugins, on Pictogrep's model but as Lua, not HTML/JS.** No WebView, so
      no second runtime: a plugin is a directory + plugin.toml (id, version,
      entry, declared permissions), run through the same Lua engine as
      scripts. Permissions enforced by what the sandbox exposes to that
      plugin's Lua state, not by a checked list. Scan shallow, skip broken
      manifests silently, reject version 0.0.0, same as Pictogrep does.
- [x] **egui is the framework, with no abstraction layer under it.** UI code
      uses egui's own types directly. The owned drawing-trait seam was
      considered and rejected as the overengineered option: 300 lines of
      indirection carried for three years against a port that is not planned.
      egui is also *why* cross-platform is affordable, since eframe carries
      winit and wgpu. Accepted cost: Lilyfield's polish ceiling is egui's
      ceiling. §13 is unaffected, layout still comes from data, not from
      egui's panel call order. (ARCHITECTURE §12)
- [x] **Cross-platform is a goal, Windows is first-class.** Reason: the only
      realistic audience (Navy Lily students) does not run niri. The cost lands
      almost entirely on pen input, since windowing and rendering come free
      with eframe. **Pen never arrives through egui on any platform**, so §5's
      out-of-band thread is required everywhere, not just on Wayland.
      Write a `PenSource` trait now, implement Wayland only. (ARCHITECTURE §0b)
- [x] **Milestone one is one finished drawing made entirely in Lilyfield.**
      Canvas + pan/zoom/rotate, pressure, one good brush, eraser, undo, save,
      export. Nothing else. The three rules still apply from line one because
      they are rules, not features. (ARCHITECTURE, "Milestone one")

## Prior art / crates to lean on
List from Tiago 2026-09-20 (originally ChatGPT-sourced, so verification status
is recorded per row: ChatGPT fabricates plausible repo names).

| Problem | Option | Status / take |
|---|---|---|
| UI | `egui` | **Verified, already in use.** |
| Desktop integration | `eframe` / `egui-winit` | **Verified, already in use.** Note eframe 0.36's trait is `fn ui(&mut self, ui, frame)`, not `update`. |
| GPU | `wgpu` + `egui-wgpu` | **Verified, already selected** (ARCHITECTURE §3, eframe wgpu backend). |
| Curves / vector paths | `lyon`, `kurbo` | **Verified on crates.io 2026-09-20** (`cargo add --dry-run` both resolve). Directly relevant to the current low-poly stroke bug: use one for curve fitting through pointer samples. |
| Brush engine | `hokusai-core` (reearth) | ✅ **VERIFIED REAL 2026-09-20, and the most important row here.** See the dedicated section below. |
| Image I/O | `image` | **Verified, already a dependency.** |
| Serious painting reference | "Arte Ogre" (visorcraft) | ⚠️ **UNVERIFIED, likely fabricated.** Same caution. |
| Tablet architecture reference | `rnote` (flxzt) | **Real project.** Highest-value reference for the step 0 pen problem: it is a Rust/GTK tablet app that solved pressure input on Wayland. Read how it gets stylus events. |
| Infinite/stroke canvas reference | `Lorien` (mbrlabs) | **Real project.** Godot, so conceptual reference only. |
| Plugin runtime | WASM/Lua approach | Lua via `mlua` already decided (ARCHITECTURE §14/§16). WASM not evaluated; note as an alternative, do not reopen without reason. |

### Hokusai: verified, and it changes the brush plan
Confirmed 2026-09-20 via docs.rs (I had wrongly flagged it as fabricated).

- `hokusai-core` 0.3.0, published 2026-09-02, MIT/Apache-2.0, repo
  github.com/reearth/hokusai, by rot1024. **Pure Rust brush engine inspired by
  libmypaint.** Native, not wasm-only (`hokusai-wasm` is just the binding).
- API: `Brush` (config), `BrushState` (per-stroke mutable state), `BrushInput`
  (input drivers), `BrushSetting`, `TiledSurface` (render abstraction).
  Handles **pressure and tilt** through `BrushInput` + a setting evaluator
  feeding the dab loop. Sibling crates: `hokusai-brush`, `hokusai-tile-mem`.
- Parses **`.myb` files**, MyPaint brushes. That is a whole existing
  ecosystem of good brushes, and it is exactly ARCHITECTURE §4's "brushes are
  data, edited as text" with the format already chosen and populated.
- This is ARCHITECTURE §4 (stamp/dab loop, scratch buffer, pressure curves)
  substantially already built, by someone who ported libmypaint's hard-won
  behavior. Strongly worth evaluating before hand-rolling the brush engine.

⚠️ **Conflict to resolve before adopting:** hokusai uses **64×64 RGBA fix15**
tiles (libmypaint's format). ARCHITECTURE §1/§2 decided **128×128 RGBA16
premultiplied linear**, and called the pixel format "the decision most
expensive to reverse". Adopting hokusai means either taking its tile/pixel
model (and rewriting §1/§2) or writing a `TiledSurface` impl that bridges to
ours (and eating a conversion cost in the hot path). Decide deliberately, do
not drift into it.

**Analysis 2026-09-20, recommendation only, needs Tiago's call since §2 calls
this the most expensive decision to reverse.** The conflict is smaller than it
looks, and splits into two independent questions:

1. **The pixel format** (irreversible, decide now). fix15 *is* premultiplied
   u16; the only difference from "RGBA16" is a full-scale of `1<<15` instead
   of 65535. That scale is the better one for a compositor: `a * b >> 15` is
   exact and needs no divide, where 0..65535 needs a divide or an
   approximation. libmypaint chose it for that reason.
2. **The tile size** (cheap, and 64 looks better on its own merits). 64×64
   RGBA16 is 32 KB, so a source and destination tile together are 64 KB and
   sit in L2. At 128×128 that is 256 KB for the pair, which does not.
   Smaller tiles also mean tighter dirty regions and less upload. The original
   argument for 128 was per-tile bookkeeping, and §3 already sends huge soft
   brushes to the GPU later anyway.

**So the recommendation is to adopt fix15 and 64×64 on their own merits,
which happens to dissolve the conflict.**

**Whether to depend on the hokusai *crate* is a separate and much less urgent
question**, because §3 already made the brush engine swappable ("given a
stroke segment, return the dirty tiles"). Deciding factor to check before
adopting: §4 requires a **two-pass composite** (whole stroke into a scratch
buffer at full alpha, composited once at stroke opacity), and libmypaint
traditionally dabs directly instead. If hokusai inherited that, adopting it
means abandoning §4's stated differentiator. Verify that specific behavior
first; take the `.myb` format regardless, since it is §4's "brushes are data"
with the format already chosen and an ecosystem already populated.

- [x] Evaluate `hokusai-core` against hand-rolling §4. Read `TiledSurface`.
- [x] Resolve the 64×64 fix15 vs 128×128 RGBA16 conflict above, explicitly.

### Resolved 2026-09-20: adopt hokusai-core, 64×64 fix15

Verified against docs.rs (`surface::TiledSurface`), not just the crate's prose:

```rust
// required
fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels
fn tile_request_end(&mut self, tx: i32, ty: i32)
// provided (overridable)
fn begin_atomic(&mut self)
fn end_atomic(&mut self) -> Vec<(i32, i32)>
fn draw_dab(&mut self, dab: &Dab) -> bool
fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels>
fn get_color(&self, x: f32, y: f32, radius: f32) -> RgbaF32
```

Only two methods are required. `draw_dab` has a default body but is
overridable, and tile storage itself is ours (`tile_request_start` hands back
`&mut TilePixels` from our own map) — hokusai never dictates tile size or
format through the trait.

**The two-pass requirement (§4) does not conflict with hokusai.** Hokusai's
ported `mypaint_brush_stroke_to` calls `self.draw_dab(&dab)` through the trait
object; it has no opinion on when that write becomes visible. Lilyfield
implements `TiledSurface` itself, dual-mode: during a stroke, backed by a
scratch buffer at full alpha sized to the stroke's bounding tiles; at stroke
end, composited once into the document's real tiles at stroke opacity. Hokusai
supplies dab generation only (spacing, pressure/tilt curves, jitter) —
compositing timing stays ours, exactly as §4 specifies.

**§3's "stroke segment → dirty tiles" interface is not precluded — it already
exists in hokusai as `end_atomic() -> Vec<(i32, i32)>`,** bracketed by
`begin_atomic`. That's a near-exact match to a decision made independently,
which is a good sign the crate is being read correctly.

**Risk vs. value:** single-maintainer (reearth/rot1024), 0.3.0, published this
month — real API-churn risk as an early adopter. Value: libmypaint-faithful
pressure/tilt/spacing/jitter math (the hard, unglamorous part of a brush
engine) plus a populated `.myb` ecosystem, satisfying "brushes are data" for
free instead of by hand. The trait boundary is narrow (2 required methods,
rest overridable), so depending on it is cheap to reverse: if hokusai stalls,
Lilyfield keeps its `TiledSurface` impl and swaps the dab-generation source
behind it, untouched compositor/undo/upload path.

**Decision: adopt hokusai-core as the dab-generation/evaluator engine.
Lilyfield owns a custom `TiledSurface` impl (scratch-buffer during a stroke,
document tiles otherwise). Do not hand-roll §4's stamp math.** This only
changes if reading `Brush`'s actual stroke-driving entry point (not yet done —
next step) shows the evaluator can't be decoupled from a `TiledSurface` at
all, i.e. no way to intercept `draw_dab` without a full realtime document
write. Check this at build-order step 4, not before — hokusai only enters
there, gated behind the mouse-testable steps 2–6, so this does not block the
pen spike or the command layer.

**Tile format: 64×64, fix15, replacing §1/§2's 128×128 RGBA16** (written into
ARCHITECTURE.md). Not a new decision so much as adopting the scale libmypaint
already chose for the reason libmypaint chose it: same bit width, cheaper
alpha math (`a*b>>15` exact, no divide), smaller tiles fit an L2-resident
src+dst pair, tighter dirty regions. Nothing else in §1/§2 changes.

Next steps, in order:
- [ ] Read `Brush`'s actual public stroke-driving method (source, not
      docs.rs prose) — confirm it takes `&mut dyn TiledSurface` and whether
      `BrushInput` is fed per-sample or pre-batched, before writing step 4.
- [ ] Verify or discard the remaining ⚠️ row (Arte Ogre) before citing it.
- [ ] Read `rnote`'s stylus input path, most direct prior art for step 0.
- [ ] Use `kurbo` or `lyon` to fit curves through pointer samples (low-poly fix).

## Brush editing
- [ ] **Brush parameters editable two ways: text config, and a synth-style
      modular/patch-cable editor.** Raised 2026-09-20. ARCHITECTURE.md section
      4 already says brushes are data (spacing, size/opacity curves against
      pressure, tilt response, jitter) and section 12b says a text/Lua editor
      is how a power user edits one. Add: also expose the same parameters as
      nodes with patchable connections (think Eurorack/synth patching:
      pressure out, size in, a curve module between them), which is not the
      same thing as the node-based *brush editor* that section 12b/16 cut.
      That one built whole brush behavior graphically as the primary path.
      This is patching existing named parameters together, still data, not a
      new execution model, so it may not violate the "no node editor" call.
      Needs a real design pass on that distinction before building either
      editor; the text-config side can ship first since it is section 4's
      plan already.
      **Design pass done 2026-09-20, and it resolves itself.** The test that
      separates the two: *does the graph add expressive power the flat file
      lacks?* If yes, the graph becomes the source of truth and the text file
      degrades into a serialization of it, which is the cut node editor back
      through the side door. If no, it is a view.
      MyPaint's `.myb` format is already exactly `input -> curve -> setting`
      with a closed set of inputs (pressure, speed, tilt, direction, random)
      and a closed set of destinations. That **is** a patch bay, with the
      cables already in the data model. So the editor is a rendering of a flat
      file, no graph runtime, no new execution model, and §12b is satisfied.
      Rule to hold: **the brush file is the source of truth and is flat. If a
      patch cannot be written as a line in that file, the patch cannot exist.**
      It is also a §13 element and a natural first plugin rather than core,
      which makes it a good proof that the primitive system is real.

## Audit fixes (2026-10-05)
- [ ] Pen stroke stuck down: proximity-out drops focus before the up/frame is sent (pen.rs ~214, 230)
- [ ] Reset pending sample on proximity-in; per-tool pending state
- [ ] Join pen thread before window/display drop (use-after-free at exit)
- [ ] Repaint every 8ms forever: pen thread should call ctx.request_repaint(); bind synchronously and report failure
- [ ] Real pen/mouse timestamps instead of dtime = 1/200
- [ ] Mouse loop restamps previous point (main.rs:190, start at 1)
- [ ] linear to sRGB on display (main.rs:24-33)
- [ ] Export .expect panics + blocks UI (main.rs:159)
- [ ] Release profile (lto fat, codegen-units 1, panic abort, strip); binary is 26 MB
- [ ] Add vulkan-loader to flake runtimeLibs; drop xorg libs
- [ ] Drop unused deps (serde_json, toml, quick-xml, zip); image png-only; use eframe::egui
- [ ] 3 clippy warnings; destroy removed tablet tools; bounded pen channel
- [ ] First-frame upload: solid paper image, not 480k HashMap lookups

## Decided 2026-10-05 (Q&A)
- [x] **No "nothing asks a question" refusal.** Rejected by Tiago. Dialogs/confirmations allowed where they help.
- [x] **Canvas flip: yes, one key.**
- [x] **Milestone one gets basic layers** (add, delete, reorder, opacity; no blend modes).
- [x] **Default canvas: A4 at 300 DPI** (2480x3508), opened with no new-file dialog.
- [x] **Rotate: keys in 15° steps.** **Pan/zoom: Space+drag, scroll zoom.**
- [x] **Eraser: toggle key E.**
- [x] **Color: always-visible docked picker + eyedropper key + fixed palette swatches.**
- [x] **Native format: own .lily zip.** **Autosave: silent snapshot every N seconds.**
- [x] **Smoothing: off by default, key to set.**
- [x] **Audience: Tiago only for now. Linux only for now** (supersedes "Windows first-class"; keep the PenSource trait).
- [x] **Milestone-one brushes: import MyPaint .myb** via hokusai.
- [x] **Default theme: light.**
- [x] **§17 approved** (handles, commands return values, one stroke command per frame).
- [x] Ask Tiago more questions about code architecture (asked 2026-10-05).
- [x] **Undo: fixed 100 steps.** **Painting on UI thread now**, move later on a profile.
- [x] **One crate, modules.** **XDG paths.** **Linear light blending** (add sRGB display step).
- [x] **No panics in app code**; errors go to the status line.
- [x] **Config = full Lua, sandboxed** (2026-10-05). REVERSES "config is parsed data, never executed". Keep it reproducible by giving the config sandbox no I/O, no clock, no randomness, and a step limit, so the same file always yields the same state. Config runs once at load and returns a table; it still never mutates state at runtime. Update ARCHITECTURE.md + the "Three rules" item to match.

## Still open
- [ ] What is Lilyfield's deliberate omission, the way Our Paint refuses to
      flip the canvas? An app with this much opinion in the architecture should
      have one in the interface.
      **Recommended 2026-09-20, not confirmed: nothing in Lilyfield can ask
      you a question.** No modals, no confirmations, no save prompt, no
      blocking file picker. It costs something real, which is what makes it a
      refusal rather than a slogan: destructive commands cannot confirm, so
      they must be undoable instead, and open/save become a summoned
      path-filtering palette. It is also just the generalisation of two calls
      already made (§1 refuses the new-file dialog, §8 refuses save-on-quit).
      Does **not** settle canvas flip: Lilyfield should probably have flip,
      since borrowing Our Paint's refusal is not the same as having an
      opinion. (ARCHITECTURE §12)
- [ ] **Primitives, not a plugin API. Design pass done 2026-09-20, awaiting
      sign-off.** Written up as ARCHITECTURE §17, marked "recommended and not
      yet confirmed". The short version:
      - §2 already committed half of it (commands are the verb layer). The
        missing half is **nouns**: commands must take **handles**
        (`layer/3`, `buffer/ref-7`) instead of reading implicit globals, or
        the command list silently becomes the feature list and the plugin-API
        shape arrives by default.
      - **Handles are capabilities.** A plugin can only act on handles it was
        handed and cannot forge one from a string, so permissions become
        "which handles do you receive at load", enforced by construction.
        The manifest's permission list becomes a request, not a runtime check.
      - Minimum version is small on purpose: a `Value` type, `(kind, id)`
        handles, a registry per kind, one `dispatch() -> Result<Value>`.
        Commands must **return** values or nothing composes.
      - Cuts from Tiago's original list: **stroke is not a primitive** (it is
        a path value; §1 decided strokes are not the document), **no timers**
        (that is how the latency premise dies), **no general file access**.
      - Found in the process: a dispatch per pen sample is 200/sec and puts
        Lua in the inner loop. **A stroke is one command, not N**: drain the
        queue and dispatch once per frame with the whole path. Also lines the
        dispatch boundary up with the undo record boundary (§7).
      - [ ] Sign off (or reject) §17 before writing step 2.

## Later
- [ ] Dirty rects and tiling tuned for large documents
- [ ] Move rasterization off the UI thread
- [ ] Rayon over tiles, once a brush is big enough to need it
- [ ] SIMD, against a profile, never before one

## Today (2026-10-06)
- [x] **Geist Sans for the whole UI** (Tiago, 2026-10-06). Embedded Geist-Regular and Geist-Bold (nixpkgs geist-font 1.5.0, OFL-1.1) replace Liberation Sans in `src/theme.rs`. Liberation files kept on disk, no longer used.
- [x] **Increase/decrease steps are logarithmic, not linear** (Tiago, 2026-10-06). Brush size and opacity keys step multiplicatively (x1.25 / x0.8). Brush opacity and layer opacity sliders map on a log scale from 0.01 to 1 (`opacity_position` in `src/chrome.rs`). Size was already log. Full test suite passes.
- [x] **One canonical keybind file** (Tiago, 2026-10-06). `~/.config/violet/keys.toml` is the only source of keybinds: the built-in table in `src/keymap.rs` is gone. Written from `assets/keys.toml` on first launch if missing. Invalid lines are reported on stderr and skipped. Edits apply live (polled every 300ms, no restart), like a Neovim config. Tests: `keymap::tests` (3 pass).
- [ ] **Fix violet enough to draw on it today** (Tiago, 2026-10-06). Smallest working fix; no restructure.
- [x] Launch diagnosed 2026-10-06: running ./target/debug/violet outside `nix develop` fails with WinitEventLoop NoWaylandLib (libwayland not on library path). Inside `nix develop -c cargo run` it stays running. Launch via `nix develop -c cargo run`.
- [x] Fuzzel launch fixed 2026-10-06: ~/.local/bin/violet builds target/fast/violet if missing, then runs it. Confirmed opening by Tiago.
