# Lilyfield architecture

Desktop drawing app. Rust + egui. Must work with Wacom tablets.

Written 2026-09-20, before any app code exists. Every choice below is made
against the requirement list: very low input latency, tablet events, GPU
rendering, image buffers, memory layout, potentially SIMD, threading, large
documents, custom rendering.

---

## What Lilyfield is

**The Neovim of drawing apps.** Super easy to get work done, not to tinker.
Opens instantly. Does not get in the way. An absolute tool.

This is a filter, not a slogan, and it has teeth. Three things follow.

### Startup time is a hard budget, not an aspiration

"Opens instantly" is measurable, so measure it. **Budget: under 150ms from
`exec` to a canvas you can draw on.** Track it in a script, run it often, and
treat a regression as a bug rather than as weather.

What threatens it, in rough order: GPU device and pipeline creation, shader
compilation on first run, font loading, and anything that touches the network
or the filesystem beyond the document. The practical rules are that **nothing
speculative happens before the first frame**, and **no feature gets to put
work on the startup path** because it would be convenient there.

That last rule immediately costs the Pictogrep integration its startup probe.
See section 11.

### The keyboard is the interface, so commands are the architecture

The Neovim lesson worth taking is not modes. It is that **your hands never
travel**. In a drawing app the pen hand is on the tablet and the other hand is
on the keyboard, so the thing to eliminate is the trip to a toolbar, not the
existence of a toolbar.

Architecturally that means **a command layer sitting between input and the
document**. Every action is a named command. Keybindings map to commands, and
the small amount of UI that exists dispatches the same commands. Nothing in
the UI calls a document method directly.

This is normally the kind of indirection worth refusing. Here it is the
product: remappable keys, a command palette, and later repeat or macros all
fall out of it, and retrofitting it means rewriting every call site. Build it
on day one.

**Recommended and not yet confirmed: not modal.** The pen always paints, so
there is no insert mode to leave. Left hand taps single keys for tool, size,
color, undo. Modes would buy ambiguity and nothing else.

### "Not to tinker" means zero *required* configuration

This looked like a contradiction at first. Lilyfield is also meant to be
completely modular, deeply scriptable, and beautifully themeable, and those are
the most tinker-shaped features imaginable.

Neovim is the proof that they coexist, and it shows exactly where the line
sits. Neovim is endlessly configurable and it is still the fastest way to get
text work done, because **none of that configuration is on the path between you
and the work**. You can move everything, script everything, theme everything.
You are never made to, and you are never interrupted by the possibility.

So the rule is not "few features". It is:

- **The defaults are so good that the config file is optional.** Anything that
  only works after you configure it is not shipped.
- **Configuration is a file, not a surface.** It lives in text, read at launch,
  and occupies no pixels while you paint.
- **Nothing configurable is ambient.** No drag handles on panels, no settings
  gear in the corner, nothing you can nudge by accident mid-stroke. You enter
  configuration deliberately, by command, or you do not encounter it.

**Out, by this rule:** preferences dialogs, onboarding, tips, modals, settings
gears, ambient drag handles, and **the node-based brush editor**. That last one
is named as inspiration in `TODO.md`, so it is being cut out loud rather than
quietly. A node graph is a GUI whose purpose is to be fiddled with, and
scripting covers the same ground without occupying the screen: **brushes are
data, and the editor for them is a text file or a Lua script.**

---

## 0. The one existential risk: the pen

egui has no pressure field in its input API, and winit does not implement the
Wayland tablet protocol. So on your niri session, **nothing in the egui stack
gives you pen pressure today**. This is not a detail to discover in week three.

**Why there is no position either, not just no pressure.** Wayland does not
synthesise pointer events from tablet input the way X11's core pointer does.
libinput classifies a device with `INPUT_PROP_DIRECT` and `ABS_PRESSURE` as a
tablet tool, the compositor routes it over `zwp_tablet_v2`, and a client that
never binds that protocol receives nothing at all. That is the whole failure,
and it matches what the hardware showed on 2026-09-20.

Three ways out, revised 2026-09-20 after two of them turned out to be wrong as
originally written:

1. **Bind `zwp_tablet_v2` yourself, on winit's own connection.**
   ⚠️ **Not a second connection.** The original plan here said to open a
   separate `wayland-client` connection, and that cannot work. Wayland object
   IDs are per-connection, and `zwp_tablet_tool_v2::proximity_in` carries a
   `wl_surface` the compositor only sends to the client owning that surface. A
   second connection owns no surfaces, so it never takes tablet focus, never
   gets `proximity_in`, and therefore never gets motion, pressure or tilt,
   since those only arrive between proximity in and out.

   The working shape shares winit's display, so you are inside the same object
   namespace: take the `wl_display` and `wl_surface` pointers from
   `raw-window-handle`, build a `Backend::from_foreign_display`, wrap it in a
   `Connection::from_backend`, create **your own event queue** on it (which is
   what §5's dedicated pen thread wants anyway), bind
   `zwp_tablet_manager_v2`, and compare the surface in `proximity_in` against
   winit's surface wrapped as a proxy on that connection. `wayland-client`
   0.31 and `wayland-protocols` 0.32 are already transitive dependencies, so
   this adds no new crate.

2. **SDL3's pen API, replacing eframe but not egui.** Worth evaluating before
   (1), because §0b made cross-platform a goal and SDL3 aims to cover Windows
   Ink, macOS and Linux pens behind one interface. **Committing to egui (§12)
   is not the same as committing to winit:** egui takes `RawInput` and returns
   shapes, and eframe is only the winit plus wgpu wrapper. So egui + wgpu +
   SDL3 honours §12 exactly, costs roughly 400 lines of input and painter
   glue once, and would replace three future platform backends with zero.
   **Unverified:** whether SDL3's Linux backend actually implements
   `zwp_tablet_v2`. Check that before anything else, it is an hour.

   This swap is cheap now, at 82 lines of app, and a month in year two. If it
   is ever going to happen it happens before the canvas.

3. **Read evdev directly.** ⚠️ **Demoted from fallback to last resort**, on a
   harder objection than the one originally written here. The problem is not
   that mapping tablet area to screen to window is tedious, it is that **a
   Wayland client cannot learn its own position on screen**, by deliberate
   protocol design. The screen-to-window half of that mapping is therefore
   impossible from inside the app. evdev can only serve as a *pressure* source
   correlated by timestamp against positions arriving through another channel,
   which desyncs. Keep it for the case where a device is not exposed over
   `zwp_tablet_v2` at all.

4. Patch winit upstream. Not for a prototype, though note that if (1) works it
   is substantially the same code as the winit PR.

**Order: verify SDL3 (2), then fall back to (1).** Either way, both sit behind
§0b's `PenSource` trait.

**Go criteria, fixed in advance so the result is not negotiable afterwards:**
pressure arrives; coordinates land where the nib actually touches; events stop
when the window loses focus; the sample rate is the tablet's, not the frame
rate.

**Do this spike before building the canvas.** A hundred lines that open a winit
window under niri and print live pressure values from a real Wacom. If that
does not work, the egui premise needs revisiting, and you want to know that on
day one rather than after the canvas is built. It is the only hardware-blocked
item in the plan, and no tablet was connected on 2026-09-20.

## 0b. Cross-platform is a goal, so the pen is a trait

**Decision 2026-09-20: cross-platform is a goal, not an afterthought.** Windows
is a first-class target. The reason is not portability for its own sake: it is
that Lilyfield has a potential audience already (Navy Lily's students) and none
of them run niri.

The consequence lands almost entirely on input, because §12's egui commitment
already carries windowing and rendering everywhere through winit and wgpu.

**Pen input never comes through egui, on any platform.** egui's input API has no
pressure field, so this is not a Wayland problem that Windows happens to solve.
On every target, pen samples arrive out of band, on their own thread, into §5's
ring buffer. The platform work is writing that source once per platform:

| Platform | Source | Notes |
|---|---|---|
| Wayland | `zwp_tablet_v2` via `smithay-client-toolkit` | §0's plan. Pressure and tilt, surface-relative. |
| X11 | XInput2 valuators | The other half of Linux, and cheaper than it looks. |
| Windows | Windows Ink (`WM_POINTER`) or Wintab | Wintab is what art apps ship; Ink is the modern API. Most Wacom drivers expose both. |
| macOS | `NSEvent` tablet events | Pressure and tilt are native to the event type. |

So the seam is one trait, not four codebases:

```rust
trait PenSource {
    fn drain(&mut self, out: &mut Vec<PenSample>);
}
struct PenSample { pos, pressure, tilt, time, tool }
```

**Write the trait now, implement Wayland only.** The trait is an afternoon. The
cost of skipping it is that §5's stabilization and latency tuning get written
against one platform's event shape and then re-derived per platform later, which
is the expensive kind of retrofit because it is tuning, not plumbing.

**Check winit first.** winit may already deliver pressure on Windows and macOS
even though it does not on Wayland. Verify before writing anything; one of those
rows may be free.

**Honest constraint:** testing Windows tablet input needs a Windows machine with
a tablet attached. Milestone one is Linux-only and that is fine. The trait exists
so that Windows is later an implementation rather than a re-architecture.

---

## 1. What the document actually is

**Decision: a sparse grid of raster tiles. Strokes are not the document.**

A stroke list is tempting (cheap undo, crisp at any zoom) and it is wrong for a
paint app. True erase, blend modes, smudge, and textured brushes all need real
pixels, and replaying ten thousand strokes to redraw is O(everything). Vector is
for an illustrator; this is a painter.

Canvas rotation and zoom are a **view transform sampling the raster**, not a
re-render of geometry. That is what every raster paint app does.

- **Tile size: 64x64. Revised 2026-09-20** (was 128x128), to match
  `hokusai-core`/libmypaint (see TODO.md, "Hokusai" section, resolved
  2026-09-20). A source and destination tile pair at 64x64 is 64 KB and sits
  in L2; at 128x128 that pair is 256 KB and does not. Smaller tiles also mean
  tighter dirty regions and less upload. 256x256 wastes upload bandwidth on
  small strokes.
- **Sparse storage**, keyed by tile coordinate. Untouched areas cost nothing,
  which means infinite canvas is later a flag rather than a rewrite.
- **Fixed document bounds. Decided 2026-09-20.** Save, load, and export all get
  an obvious answer to "how big is it". Sparse tiles are kept anyway, because
  blank areas then cost nothing and infinite canvas stays a later flag rather
  than a rewrite.
- **But never ask at startup.** Fixed bounds must not become a new-file modal
  asking for width, height, and DPI before you can draw. Launching drops you
  straight onto a canvas at a sane default size. Resizing is a command, used by
  the people who need it, on the rare day they need it. A dialog between the
  user and the first stroke fails the whole premise.

## 2. Pixel format

**Decision: fix15 (u16 per channel, scale `1<<15`), premultiplied alpha,
linear light. Revised 2026-09-20** (was RGBA16 at scale 65535) to match
`hokusai-core`/libmypaint (see TODO.md, "Hokusai" section, resolved
2026-09-20). Same bit width and same "most expensive to reverse" status as
before; fix15 is just the scale libmypaint chose because `a * b >> 15` is
exact and needs no divide, where 0..65535 needs one.

8-bit sRGB is the cheap option and it fails at exactly the thing this app is
for. Pressure-sensitive painting stacks many low-opacity strokes, and 8 bits
bands visibly in the darks after a dozen passes. Blending in sRGB also gives
muddy fringes on colored blends.

16-bit doubles memory (a 4000x4000 document is 128 MB, which is fine) and
integer u16 vectorizes cleanly. Convert to sRGB 8-bit only at display upload.

This is the decision most expensive to reverse, since it touches every line of
the compositor, which is why it is being made now rather than deferred.

**Pigment mixing: no, for now. Decided 2026-09-20.** Our Paint's headline
feature is a pigment mode that simulates real color theory instead of RGB
blending. Lilyfield is RGB.

Worth being straight about what "for now" buys and costs. Convincing
subtractive mixing (Kubelka-Munk) works on *spectral* data, so it needs more
than four channels per pixel: a different pixel model, a different memory
budget, a different compositor. Adding it later is a compositor rewrite, and no
amount of foresight makes it cheap.

The one thing that genuinely helps, and costs nothing today, is **keeping all
channel math behind a single compositor module** rather than letting blend
arithmetic scatter across the brush, the eraser, and the layer stack. Then a
later pigment mode is one module replaced instead of a search across the
codebase. Do that much, and nothing more.

## 3. Rendering path

**Decision: CPU stamps into tiles, upload dirty tiles, egui draws textured
quads. Backend is wgpu, not glow, from day one.**

Run the numbers before reaching for shaders: a fast stroke dirties maybe four
to ten tiles per frame, so ~600 KB of uploads per frame. That is nothing. CPU
stamping at a few hundred stamps per second over small radii is also nothing.
The simple path is fast enough, and it is far easier to debug.

Where it stops being fast enough is huge soft brushes (a 500px airbrush is
780k pixels per stamp). So structure the brush engine's interface as **"given a
stroke segment, return the dirty tiles"**, never as "write into this Vec". That
keeps a GPU stamping pass a drop-in replacement instead of a rewrite.

Choose the **wgpu** eframe backend now even though nothing needs it yet. Custom
render passes go through `egui::PaintCallback`, and those are backend-specific,
so switching later means rewriting whatever custom rendering exists by then.

## 4. Brush engine

Stamp-based: walk the path at a spacing of roughly 0.1 to 0.25 of the radius,
interpolating pressure between input samples, accumulating leftover distance
across segments so spacing stays continuous around corners.

**The non-obvious requirement: one stroke composites in two passes.** Stamps
overlap heavily, so stamping them one by one at 20% opacity double-darkens
everywhere they overlap and the stroke comes out blotchy with a dark edge where
you changed direction. Real brushes render the whole stroke into a **scratch
buffer at full alpha first, then composite that buffer once** at the stroke's
opacity. This is the difference between a real brush and MS Paint, and it has
to be in the pipeline from the start because it dictates where the eraser and
the undo snapshot hook in.

The eraser is then just the scratch buffer composited destination-out. Same
code path, one flag.

**Brushes are data, not code.** A brush is a deserialized struct (spacing, size
and opacity curves against pressure, tilt response, jitter), not a hardcoded
function. That costs nothing today and is how good defaults ship and how a
power user edits one in a text file.

To be clear about what this is *not* building toward: the node-based brush
editor is cut, for the reasons at the top of this document. Brushes are data so
that the defaults can be good, not so that an editor can exist.

## 5. Input and latency

The part that decides whether the app feels good.

- **Pen events arrive on their own thread.** A Wacom reports around 200 Hz; the
  display runs at 60 or 120. Sampling the cursor once per frame throws away
  most of the stroke and gives you faceted, laggy lines.
- **Timestamped samples go into a ring buffer. The UI thread drains the entire
  queue each frame** and stamps every sub-frame segment, rather than reading a
  single current position.
- **Stabilization** (a one-euro filter, or pulled-string smoothing) runs on the
  queue before stamping. Raw pen input is jittery. Keep it one function so it
  stays tunable.
- **`request_repaint()` continuously while a stroke is live.** egui is immediate
  mode and otherwise idles between events.
- **Honest expectation:** on Wayland you cannot do the front-buffer tricks that
  native Wintab apps use, so realistically you land one to two frames behind the
  pen. Worth knowing the floor so you do not spend a week chasing it.

## 6. Threading

Where threads actually earn their complexity:

- **Pen input thread.** Mandatory, per above.
- **Per-tile stamping via rayon.** Tiles are independent by construction, so
  this is embarrassingly parallel. Only pays off for large brushes; wire it when
  a brush gets slow, not before.
- **Save, load, and PNG encode off-thread.** Trivial to do, keeps the UI from
  hitching on every autosave.

Where they do not: **the document itself stays single-writer.** One thread
mutates tiles. Do not build a concurrent document model.

## 7. Undo and redo

**Decision: per-stroke copy-on-write tile snapshots.**

Before a stroke first touches a tile, clone that tile into the stroke's undo
record. Undo swaps the old tiles back. Cost is proportional to the area you
actually painted, which is the correct shape. Command-log replay is O(document
history) and full snapshots eat memory; this is what Krita and Procreate do.

With tiles behind `Arc`, snapshots are cheap and the same mechanism later gives
you async saving without blocking the painter.

Bound the undo stack **by bytes, not by count**, and evict oldest.

## 8. Save, load, export

- **Save and export are different operations.** Save round-trips the document;
  export flattens to PNG or JPEG for sharing. Do not let one button do both.
- **Native format: a zip containing `manifest.json` plus one 16-bit PNG per
  layer.** Inspectable, compressed for free, no precision loss, and you can
  recover a file by hand with `unzip` if the app ever corrupts something.
- **Never write in place.** Write to a temp file, fsync, rename. Same invariant
  milklily holds about original footage: a crash must never be able to eat work
  that was already on disk.
- Autosave on the same mechanism, to a separate recovery path.

**A tool does not lose work, and does not nag about it.** Two rules fall out of
the philosophy. Nothing is ever lost to a crash, a closed lid, or a power cut,
which is what autosave and the recovery path are for. And Lilyfield never puts
up a "do you want to save your changes?" dialog on quit. It saves, and it
quits. The dialog exists in other apps to protect against a data model that
cannot recover, and this one can.

## 9. Layers

**Decision: layers exist in the data model from day one. The prototype ships
with exactly one layer and no layer UI.**

This is the one place deliberately paying for the future, and it is worth
saying why, given the standing rule is to build the smallest thing. Layers
change the compositor (a blend stack instead of one buffer), the undo records
(tile keys become layer plus coordinate), and the save format. Adding them to a
single-buffer app later is a compositor rewrite. Adding an extra index to a
struct today costs nothing.

## 10. SIMD

**Decision: write no SIMD now.**

The layout decisions above *are* the SIMD decisions. Contiguous rows, premultiplied
channels, interleaved RGBA16, tiles that fit in cache: that is what makes
vectorization possible. Write the composite inner loop as branch-free scalar
code over `&mut [u16]`, let the autovectorizer have it, and measure.

Reach for `std::simd` or the `wide` crate only against a profile, on the
specific inner loop that shows up. Hand-written intrinsics written today would
be optimizing a compositor whose shape is still unknown.

---

## 11. Pictogrep integration

**Decision: Lilyfield is a native HTTP client of Pictogrep's local server. It
is not a Pictogrep plugin.**

That is forced, not chosen. A Pictogrep plugin is a directory of HTML and JS
loaded into an iframe inside Pictogrep's WebView, with a fixed permission list
(`images.list`, `images.tag`, `storage.kv`, `ui.panel`). A native Rust painting
app with its own GPU surface cannot live in that sandbox, and should not want
to. The integration runs the other way around.

Equally: **do not embed a WebView in Lilyfield** to reuse Pictogrep's UI. That
drags a whole browser engine into an app whose entire premise is low latency.

### What Pictogrep already exposes

Everything needed is in `server.go` today, no new surface required:

- `GET /api/app/state` for discovery (version, paths)
- `GET /api/app/search?q=...` for the plain-language search, which is the
  actual reason to integrate
- `GET /thumbnail/{id}` and `GET /image/{id}` for pixels
- `GET /api/app/boards` and `GET /api/references` for curated sets
- `POST /api/app/index` and `GET /api/app/folders` for pushing work back in

### The feature that justifies it

A **reference panel docked beside the canvas**: type what you half-remember,
get your own library back, pin a few images next to the painting. That is
Pictogrep's whole value proposition pointed at a canvas, and it removes the
alt-tab that currently sits in the middle of every drawing session.

Second, smaller: **finished work saves into a Pictogrep-indexed folder**, so
your own paintings become searchable reference later. The loop closes.

### The blocker, and it needs your consent

Pictogrep authenticates local callers with `PICTOGREP_TOKEN`: a secret the
shell invents fresh on every launch and passes to the Go process by
environment variable, checked as an `X-Pictogrep-Token` header or a cookie.
**A separate process cannot read an env var of an already-running process.**
When no token is set the API is open, so a hand-launched `pictogrep` works
today, but the normal desktop launch does not.

The fix is small and standard: Pictogrep writes a `session.json` into its data
home at startup, mode 0600, containing the token and the **actual** port. The
port matters independently, because Pictogrep falls back to a random port when
8765 is taken, and right now nothing publishes which port it landed on.

That is a change to the **public pictogrep repo**. Asked on 2026-09-20, and the
answer was to ask again at implementation time. So: plan the integration fully,
touch nothing in pictogrep, and raise it again when the reference panel is
actually being built.

### Non-negotiable constraint

The integration is optional and fails silently. Lilyfield is a complete
painting app with Pictogrep absent, uninstalled, or closed. Never block a
stroke on a network call.

**And the probe does not happen at startup.** An HTTP request to a port nobody
is listening on is exactly the kind of speculative work the startup budget
forbids, and a connection timeout on the launch path would be the single
worst-feeling bug this app could ship. Discovery happens lazily, the first time
the reference panel is opened, and its result is cached for the session.

## 12. UI direction

The reference is Our Paint: a custom GUI framework with 3D elements and
animation, and a level of polish that is the point of the app rather than a
coat on top of it.

**Be honest about egui here.** egui's defaults look like engine debug UI: flat
gray, generic. It will not approach Our Paint's finish for free. Getting there
means a committed custom `Visuals`, a real typeface, and hand-built widgets for
anything the canvas touches. egui buys fast iteration and immediate-mode
simplicity, not looks.

**Decision 2026-09-20: egui is the framework, and there is no abstraction layer
under it.** UI code uses egui's own types directly (`Ui`, `Painter`, `Response`,
`Rect`, `Color32`, `FontId`). No drawing trait, no portability seam, no plan to
leave. The alternative considered and rejected was a thin owned drawing layer
(rect, text, image, path, clip) that egui would back today and something else
could back later.

Two things make that the right call rather than a resigned one.

egui is **why cross-platform is affordable at all** (§0b). eframe carries winit
and wgpu, so Windows, macOS, X11 and Wayland windowing, rendering and event
plumbing arrive already solved. An owned GUI layer means writing text shaping, a
font atlas and four platform backends before the first stroke exists, which is
the version of this project that never ships.

And the seam was the overengineered option. Three hundred lines of indirection
carried for three years against a port that is not planned is exactly the
speculative machinery this document refuses everywhere else.

**The accepted cost, written down once so it is not rediscovered as a surprise
in year two:** Lilyfield's ceiling of polish is egui's ceiling. Our Paint wrote
its own GUI framework and will look better for it. The bet is that "chrome that
is absent cannot look generic" makes that ceiling mostly irrelevant, because the
canvas is most of the screen and the canvas is ours.

**What committing to egui does not change:** §13 still stands in full.
`TopBottomPanel` and `SidePanel` derive their nesting from the order you call
them in source, which is a layout hardcoded in code. Lilyfield still computes
rects from layout data and renders each element into its rect. Committing to
egui means using its types, not adopting its layout system.

**The philosophy mostly rescues this**, which is worth noticing, because "the
Neovim of drawing apps" deletes nearly all the surface that would have looked
generic. What is left:

- **The canvas, edge to edge.** No permanent toolbars, no docked palettes.
- **One status line at the bottom.** Tool, brush size, zoom, document name,
  modified flag. Very Neovim, and it happens to be the one widget egui draws
  well.
- **A transient HUD.** Brush size and color appear while you are changing them
  and fade out. They are feedback, not furniture.
- **Summoned, not resident.** Tool and color pickers appear under the cursor
  when asked for and vanish on use. Nothing that is needed twice a session
  gets to occupy pixels for the whole session.

Chrome that is absent cannot look generic. The egui risk shrinks to the status
line and a handful of popups, which is an afternoon of theming rather than a
rewrite of a design language.

**Adopt Pictogrep's UI blueprint verbatim** (`pictogrep/docs/ui.md`), so the
two apps read as siblings and a thousand micro-decisions are already made:

- Spacing only from 4, 8, 12, 16, 24, 32
- Button heights only 28, 36, 44, with 16px icons (20px on large controls)
- Radius only 4, 8, 12, 16
- Type only 24, 18, 14, 12, and only three weights
- Icon buttons never shrink to the size of the icon

Two house rules that egui actively fights, so they need explicit overrides:

- **No gray text, ever.** Hierarchy comes from weight, size, or the accent
  color. egui's default visuals lean on gray for everything secondary, so
  `weak_text_color` and friends all get replaced.
- **Chrome never competes with the canvas.** The canvas is the sacred object.
  Panels stay out of the way, and no piece of chrome ever triggers a document
  repaint.

**The open design question:** Our Paint refuses to implement canvas flip, on
purpose, because flipping lets you outsource your eye. That refusal is most of
its personality. What is Lilyfield's? An app with this much opinion in the
architecture should have one in the interface too.

**Recommended and not yet confirmed: nothing in Lilyfield can ask you a
question.** No modal dialogs, ever. No confirmations, no save prompts, no
blocking file picker, no "are you sure". There is no state in which the app is
waiting on you.

It qualifies as a real refusal because it costs something specific. Destructive
commands cannot confirm, so they have to be undoable instead, which is the better
design and this forces it rather than hoping for it. Open and save need a
non-modal answer, which is a summoned palette that filters paths, the same shape
as the command palette.

It is also the generalisation of two calls already made above: §1 refuses a
new-file dialog between launch and the first stroke, and §8 refuses the
save-on-quit prompt. Those are the same rule applied twice. Naming the rule is
cheaper than rediscovering it once per feature.

This does not settle canvas flip. Lilyfield should probably have flip, since
refusing it is Our Paint's identity, and borrowing someone else's refusal is not
the same as having an opinion.

## 12b. Config is declarative, like NixOS. Scripting is not config.

This corrects something the Neovim analogy gets wrong, so it needs saying
before layout, scripting, or themes get built.

**Requirement: reproducible.** Not "imperatively configured", entirely
configured through config files, the way a NixOS system is a function of its
flake, not of commands someone ran against it.

Neovim's `init.lua` fails exactly this test. It is a *program*, not a
description, so what your editor ends up looking like depends on load order,
on which plugins happened to be installed, on conditionals that read the
environment. Two people with the same file can land in different states. That
is the specific thing to not copy.

**So Lilyfield has two separate layers that must never merge:**

- **Config: keymap, layout, theme.** Plain data (TOML, or Lua tables that are
  evaluated once into pure data and then thrown away). Loading it is not
  running a program, it is parsing a description. The same file produces the
  same app state on every machine, every time, full stop. This is what
  "entirely configurable like NixOS" means concretely: the file *is* the state,
  not a set of instructions for reaching some state.
- **Scripts: commands, brushes, hooks.** Genuinely imperative Lua, because
  "draw a spiral" or "run this filter" is behavior, not configuration, and
  behavior has to be code somewhere. This layer is allowed to be a program.

The rule that keeps them apart: **a script may never write the config layer,
and loading config never executes Lua as a program.** A theme file is read with
a data-only parser. If a script wants to change the active theme, it calls a
command (`set-theme "solarized"`) that swaps in a different pre-parsed data
file; it does not reach in and mutate color fields at runtime. Config stays a
pure function of its files, scripts stay free to misbehave without taking
reproducibility down with them.

Practically, this also answers most of what a startup profiler (section 14)
needs to distinguish: **parsing config is expected to be near-instant and
constant-time; running scripts is the part allowed to be slow, and named as
the user's own doing when it is.**

## 13. Modular UI

**Requirement: every single UI element can be repositioned.**

The whole of it reduces to one rule, and the rule is cheap now and a UI rewrite
later:

> **No element decides where it lives.** Every element is a named unit that
> renders into a rect it is handed. Position is never expressed in the
> element's own code.

From that, three things follow.

**Elements are named and dynamic, not a compile-time list.** A script that adds
a panel (section 14) has to be able to name it, and the layout has to be able
to place something the binary has never heard of. So the registry is a map at
runtime, not an enum.

**Layout is data**, in the declarative sense of section 12b: a file mapping
element names to a region, an anchor, and a size, parsed once at launch, never
executed. It ships with a default that nobody has to touch, and the same file
produces the same layout on any machine.

**egui's panel API cannot be used the normal way.** This is the concrete
consequence and it needs saying before any UI code exists. `TopBottomPanel`,
`SidePanel` and friends derive their nesting from *the order you call them in
your source*, which is precisely a layout hardcoded in code. Lilyfield instead
computes rects itself from the layout data and renders each element into its
rect with `Area` or `allocate_ui_at_rect`. Convenient egui panel code written
early is the thing that would have to be torn out later.

**Repositioning is a mode, not a hover state.** You enter layout mode by
command, drag, leave, and it writes the layout file back as plain data, the
same file format you could have hand-edited. Ambient drag handles would mean
nudging your status line during a stroke, which is the exact failure the
philosophy exists to prevent.

**Worth flagging honestly:** a fully modular UI is a lot of machinery for an app
whose stated goal is to have hardly any UI. The two only stay compatible if the
element count stays small. The machinery is justified because it is cheap when
built first and impossible to retrofit; it is not an invitation to add elements
because moving them is now supported.

## 14. Scripting

**Decision: Lua, embedded through `mlua`.**

It is the Neovim answer and it is the right one here. The VM initialises in
about a millisecond, which is nothing against a 150ms budget. It is small,
genuinely fast, and an artist can paste a ten-line snippet from someone else
and have it work. The only serious alternative is Rhai, which is pure Rust and
avoids a C dependency, and trades a language everybody knows for one nobody
does. Python is out on startup cost alone.

**The rule that keeps scripting from destroying the latency premise:**

> **Lua orchestrates, Rust does pixels. The script layer never appears in an
> inner loop.**

A script says "stamp this brush along this path" or "run this filter on this
layer". It never says "set this pixel". Per-pixel bindings would be the one
scripting decision that cannot be walked back, because once people write
scripts against a slow API, the API is load-bearing.

**What scripting reaches**, roughly in order of how much it is worth:

- **Commands.** Call any of them, and define new ones. Since the UI dispatches
  commands too (section 2 of this document), a scripted command is a first-class
  one, bindable to a key and reachable from the palette.
- **Keymaps and layout.** Config becomes scripting's easiest use case rather
  than a separate system.
- **Brushes.** Define and modify them, which is the node editor's job done in
  text.
- **Strokes.** Draw programmatically. Symmetry, perspective guides, pattern
  and repeat tools, generative work. This is where a scriptable paint app earns
  its keep over a scriptable text editor.
- **Hooks.** On stroke end, on save, on document open.
- **Layers and document.** Batch operations only, per the rule above.

**Scripts must not be allowed to blow the startup budget.** This is Neovim's
best-known self-inflicted wound: a slow `init.lua` and suddenly the instant
editor takes 400ms. Borrow the fix along with the language. Load scripts lazily
where the shape allows it, and ship a **startup profiler from day one** that
attributes milliseconds to whatever spent them. If a user's script is slow,
they should be able to see that it is theirs.

## 15. Themes

**Requirement: beautiful themes, covering UI, background, and fonts.**

Colorschemes are half of why people love their editor, and this app has a
genuine advantage over an editor: there is a large beautiful surface, the
canvas surround, that a text editor does not have.

**A theme is a data file naming a closed set of tokens**, parsed, never
executed, per section 12b. Closed is the important word. If elements are free to invent their own colors, every new
element silently breaks every existing theme. A small fixed token set means any
theme works with any element, forever, including elements added by a script.

The token set covers three things, because all three were asked for:

- **UI.** Surfaces, borders, text, accent. Small enough to hold in your head.
- **Background.** The workspace around the canvas, which is most of what you
  actually look at while painting, and which affects how you judge color. Worth
  supporting a flat tone, a subtle gradient, and a texture, because a warm gray
  surround and a black void are genuinely different painting experiences.
- **Fonts.** Family, and the sizes from Pictogrep's scale.

**Encode the house rule in the token set: there is no muted gray text token.**
Hierarchy comes from weight, size, and accent. Not offering the token is a
better enforcement than remembering the rule, because it makes the wrong thing
unrepresentable rather than merely discouraged.

**Two startup-budget rules fall out of fonts:**

- **Embed the default typeface.** Do not enumerate system fonts at launch.
  Fontconfig can spend tens of milliseconds answering a question we already
  know the answer to.
- **Custom fonts load only when a theme names a path**, and are cached.

Reloading a theme is a command, not a file watcher. Same effect while authoring
one, no background thread, and it goes through the command layer like
everything else.

## 16. Plugins, like Pictogrep's

**Requirement: a plugin system on the model of Pictogrep's.**

Pictogrep's model, concretely (`pictogrep/docs/plugin-api.md`): a plugin is a
directory under a data folder, holding a `plugin.json` with an id, a version,
an entry point, and a declared **permission list**. The host scans that
directory, and a plugin only gets the capabilities it declared.

That whole shape ports over directly. What does not port over is the runtime:
Pictogrep's entry point is HTML and JS loaded into a WebView iframe, and
section 11 already ruled out embedding a WebView in Lilyfield, for the same
reason it ruled out being a Pictogrep plugin: it drags a browser engine into an
app whose entire premise is startup time and input latency.

**So a Lilyfield plugin is a directory holding a manifest and Lua**, run
through the same engine as section 14's scripts, not a second runtime bolted
on. This is not a departure from the scripting design, it is the scripting
design pointed at distribution:

| | Personal script (§14) | Plugin (§16) |
|---|---|---|
| Where | user's config directory | plugins directory, scanned like Pictogrep's |
| Identity | none needed | `id`, reverse-DNS, per Pictogrep's convention |
| Capabilities | full API | **declared permissions only** |
| Distributed | no | yes, a folder someone can share |
| UI | none, or ad hoc | can register a named element (§13) |

**`plugin.toml`, deliberately not JSON**, since everything else configurable in
Lilyfield is TOML per section 12b and a plugin manifest is config, parsed once,
never executed:

```toml
id = "dev.example.symmetry"
name = "Symmetry"
version = "0.1.0"
entry = "init.lua"
permissions = ["commands", "brushes", "canvas.read", "ui.panel"]
```

**Permissions are enforced by what the Lua sandbox exposes, not by convention.**
This is a place where Lilyfield's position is stronger than Pictogrep's: a
browser sandbox has to defend against a full JS engine and the DOM, while an
`mlua` sandbox only exposes whatever Rust functions were registered into that
plugin's Lua state to begin with. A plugin that did not declare `canvas.read`
never has a `read_pixels` global to call. There is no permission check to
forget, because the capability was never bound into the plugin's world. Denial
by construction, the same trick the theme token set already uses to make gray
text unrepresentable.

**A plugin panel is a UI element like any other**, registered by name into the
element registry (§13), and the layout file decides where it lives, exactly
like Pictogrep's `panel: { title, icon }` gets a slot in *its* UI. The
difference is Lilyfield's user can drag it anywhere, because every element can
be repositioned.

**Directly reused from Pictogrep's design, because it already earned its
shape** by being fought over in a shipped app: scan is shallow, one level
under `~/.local/share/lilyfield/plugins`; a broken manifest is skipped
silently rather than crashing the scan, so one bad plugin cannot take the
others down; version `0.0.0` is reserved and never loaded, for scaffolds with
a manifest and nothing worth running yet; two plugins claiming the same `id`
is not an error, and reverse-DNS is convention, not validation.

**What plugins do NOT get, on the same grounds as section 12 and 12b:**

- **No pixel-loop access.** Same rule as scripts: a plugin calls `stamp_brush`
  or `run_filter`, it does not get a raw framebuffer pointer. A permission
  named `canvas.write` still means "through the compositor's API", never
  "through its memory".
- **No mutating config directly.** A plugin that wants to change the theme
  calls the `set-theme` command, per section 12b, same as any script.
- **Nothing on the startup path.** A plugin's Lua does not run until either its
  panel is opened or its command is invoked, same lazy-load discipline as the
  Pictogrep reference panel in section 11. `id`, `name`, `version`, `entry`,
  and `permissions` are read from every manifest at launch, because the plugin
  list has to exist to show one; the plugin's *code* does not run until asked
  for. A directory of ten plugins costs a handful of TOML parses at startup,
  not ten cold Lua VMs.

## 17. Primitives, not a plugin API

**Recommended and not yet confirmed.** Raised 2026-09-20. It needs settling
before the command layer is written, because it reshapes §2 as much as §16.

The framing: "we support plugins" is a host with a fixed feature set plus a
sandboxed surface bolted to the side and a permission list naming which bits you
may touch. That is VSCode, the browser, and Pictogrep. "The application itself is
composed of things that can be manipulated" means the app's own features are
built from the same primitives scripts and plugins get, so a feature is a
combination of primitives rather than a special-cased code path. That is Emacs,
and it is the stronger position.

§2 already committed half of it. "Nothing in the UI calls a document method
directly, everything goes through a named command" is the primitives move applied
to **verbs**.

**The missing half is nouns.** If `dispatch("brush.stamp")` reads its target from
implicit global state ("the current layer"), a script can only do what the app
already does, in the order the app already does it. That is a macro recorder.
Every new capability then needs a new command, the command list becomes the
feature list, and the plugin manifest has to enumerate it. **The plugin-API shape
arrives by default if nothing is decided.**

So the concrete content of "addressable" is: **every primitive instance has a
stable handle, and commands take handles as arguments.** `layer/3`,
`buffer/ref-7`, `element/status-line`, `brush/ink-round`.

**The payoff: handles are capabilities.** If a plugin can only act on handles it
was handed, and cannot forge one from a string, then permissions are just "which
handles do you receive at load". A plugin with a panel gets
`element/its-own-panel` and nothing else. There is no permission check to forget,
because the capability was never bound into its world. Same denial-by-
construction trick §15 uses to make gray text unrepresentable and §16 uses for
the Lua sandbox, applied to the whole object space. The manifest's `permissions`
becomes a request list the host reads at load, not a list checked at call time.
Personal scripts (§14) get a root handle; plugins get a subtree.

**Minimum viable version, because this could easily become a research project:**
a `Value` type (nil, bool, number, string, handle, list, map), a handle that is
`(kind, id)` and prints as `kind/id`, one registry per kind, and one
`dispatch(name, args: Value) -> Result<Value>`. That is a day or two.

Commands **return** values. This is the part most command layers get wrong: if
dispatch returns `()`, nothing composes, and `doc.layers` cannot hand back a list
a script can walk without inventing new API surface for every query.

**Proposed primitive set**, deliberately smaller than the list it came from:
Document, Layer, Buffer, View, Brush, Command, Keymap, Element, Theme, Event,
Plugin. A Buffer is the pixel primitive, so a layer *has* one and an imported
reference image *is* one, rather than being two systems.

Three cuts worth arguing for:

- **Stroke is not a primitive.** §1 decided strokes are not the document. Making
  strokes addressable entities drags the model back toward vector by the side
  door. A stroke is a path *value*, constructed and handed to a command, never
  queried as `stroke/47`.
- **No timer primitive.** A plugin holding the equivalent of `setInterval` is how
  the latency premise dies quietly. If periodic work is ever needed it is a named
  frame hook, not a thread a plugin owns.
- **No general file primitive.** Arbitrary filesystem access is the entire reason
  a permission system has to exist. A plugin gets a data directory and explicit
  import and export commands, which is the call Pictogrep's `storage.kv` already
  made.

**One collision this exposes, worth writing down before §5 is implemented.** Pen
samples arrive at roughly 200 Hz (§5) and everything must go through a command
(§2). A dispatch per sample is 200 registry lookups and boxed argument sets per
second, and 200 Lua calls per second the moment anything hooks stroke input.

The resolution is that **a stroke is one command, not N.** The UI thread drains
the whole queue and dispatches once per frame with the entire path as a single
value. Sixty dispatches per second instead of two hundred, Lua stays out of the
inner loop per §14, and the dispatch boundary lands exactly on the undo record
boundary from §7. The per-sample version is the obvious implementation and it is
the wrong one.

---

## Milestone one: one finished drawing

**Decision 2026-09-20: the first milestone is one finished drawing made entirely
in Lilyfield.** Not a feature list, not a version number.

It exists because the actual failure mode of a three-year solo project is not a
wrong tile size, it is that nobody ever uses the thing. One finished piece is the
moment the tool crosses from demo to usable, and it produces the only feedback
loop available in year one: you, painting, noticing what is intolerable.

**What it requires:** canvas with pan, zoom and rotate; pen with real pressure;
one brush that feels good; an eraser; undo; save and export.

**What it explicitly does not require:** layer UI, scripting, plugins, themes,
layout mode, the Pictogrep panel, a second platform.

The three rules still apply from the first line of code, because they are rules
rather than features: no call site touches the document directly, no element
knows its own position, no element invents a color. Everything else in sections
13 through 17 waits until a drawing exists.

## Order of work

The pen is the risk, the canvas is the deliverable, and they are independent.

0. **Pen spike.** Pressure and tilt from a real Wacom inside a winit window on
   niri. Blocked on hardware. Do it first anyway.
1. **Startup measurement.** A script that times `exec` to first frame, before
   there is anything to slow down. A budget nobody measures is a wish.
2. **Command layer.** Named commands, a keymap, dispatch. Thin and boring at
   this stage, and everything after it goes through it.
3. **Canvas skeleton.** Tiles, view transform (pan, zoom, rotate), blank canvas
   on screen, dirty-tile upload.
4. **Brush.** Scratch buffer, stamping, spacing, pressure to size and opacity.
5. **Eraser and history.** Destination-out composite, copy-on-write undo/redo.
6. **Save, load, export.** Zip plus PNG, atomic writes, autosave.
7. **Wire the real pen in.** Stabilization, latency tuning.
8. **Theme tokens, then the UI.** Status line and summoned pickers, each one
   rendered into a rect handed to it by the element registry. The whole UI,
   such as it is.
9. **Layout file and layout mode.** Now that elements exist to place.
10. **Lua scripting.** Commands first, then brushes, strokes, and hooks.
11. **Pictogrep reference panel.** Lazy discovery, never on the startup path.

Steps 2 through 6 are all testable with a mouse at pressure 1.0, so the
hardware block on step 0 never stalls the build.

**Three of these are rules before they are steps**, and that distinction is the
whole plan. The command layer (2), the element registry (8) and the theme
tokens (8) are each about a day of work when nothing depends on them and a
rewrite once everything does. Their later entries on this list are only when
the *files* arrive: the keymap, the layout, the colorschemes. The constraint
that no call site talks to the document directly, and no element knows its own
position or invents its own color, applies from the first line of UI code.

Step 2 before step 3 is the specific ordering worth defending. It is tempting
to draw pixels first and add commands once there is something to command. Do it
the other way.

## Deliberately not built yet

Named so they stay decisions rather than drift: GPU brush stamping, rayon tile
parallelism, SIMD intrinsics, infinite canvas, layer UI, blend modes beyond
normal and erase, brush textures, selections, transforms, text.
