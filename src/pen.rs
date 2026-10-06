//! Real pen input, ARCHITECTURE §0/§0b: `zwp_tablet_v2`, bound on winit's own
//! Wayland connection (a second connection owns no surfaces and never gets
//! `proximity_in`). Runs on its own thread; `drain` is the only thing the UI
//! thread touches.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use eframe::egui;
use rustix::event::{PollFd, PollFlags, Timespec};

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use wayland_backend::client::{Backend, ObjectId};
use wayland_backend::protocol::Interface;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::tablet::zv2::client::{
    zwp_tablet_manager_v2::ZwpTabletManagerV2,
    zwp_tablet_seat_v2::{self, ZwpTabletSeatV2},
    zwp_tablet_tool_v2::{self, ZwpTabletToolV2},
    zwp_tablet_v2::{self, ZwpTabletV2},
};

/// The pen's state at one tablet frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct PenSample {
    /// Surface-local position, in logical points.
    pub x: f32,
    pub y: f32,
    /// 0 to 1.
    pub pressure: f32,
    /// Degrees.
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub down: bool,
    /// Compositor timestamp of the frame, in milliseconds.
    pub time_ms: u32,
}

/// Where pen samples come from; one implementation per platform (§0b).
pub trait PenSource {
    /// Appends every sample that arrived since the last call. Never blocks.
    fn drain(&mut self, out: &mut Vec<PenSample>);
}

pub struct WaylandPenSource {
    samples_rx: mpsc::Receiver<PenSample>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl WaylandPenSource {
    /// Must run before the window (and its `wl_display`) is dropped: the
    /// thread reads from winit's connection and would outlive it otherwise.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl PenSource for WaylandPenSource {
    fn drain(&mut self, out: &mut Vec<PenSample>) {
        out.extend(self.samples_rx.try_iter());
    }
}

impl WaylandPenSource {
    /// `display`/`surface` must be the raw handles of the very winit window
    /// receiving strokes; a second Wayland connection cannot see them.
    pub fn connect(
        display: RawDisplayHandle,
        surface: RawWindowHandle,
        ctx: egui::Context,
    ) -> Option<Self> {
        let RawDisplayHandle::Wayland(display) = display else {
            return None;
        };
        let RawWindowHandle::Wayland(surface) = surface else {
            return None;
        };
        // Raw FFI pointers aren't Send; carry them as addresses across the
        // thread boundary and cast back inside, where they're used the way
        // the C API always intended (a pointer valid for the process).
        let display_addr = display.display.as_ptr() as usize;
        let surface_addr = surface.surface.as_ptr() as usize;

        // Bounded so a UI that stops draining (minimised window) can't
        // queue hover samples without limit and replay them as a burst.
        let (samples_tx, samples_rx) = mpsc::sync_channel(1024);
        let (ready_tx, ready_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("violet-pen".into())
            .spawn(move || {
                pen_thread(display_addr, surface_addr, samples_tx, ctx, ready_tx, thread_stop)
            })
            .ok()?;
        // Setup is synchronous: no tablet manager means no pen source.
        if ready_rx.recv() != Ok(true) {
            let _ = thread.join();
            return None;
        }
        Some(Self { samples_rx, stop, thread: Some(thread) })
    }
}

/// Binds the tablet protocol, reports on `ready` whether that worked, then
/// pumps Wayland events until `stop` is set.
fn pen_thread(
    display_addr: usize,
    surface_addr: usize,
    samples_tx: mpsc::SyncSender<PenSample>,
    ctx: egui::Context,
    ready: mpsc::Sender<bool>,
    stop: Arc<AtomicBool>,
) {
    let display_ptr = display_addr as *mut std::ffi::c_void;
    let surface_ptr = surface_addr as *mut std::ffi::c_void;
    // SAFETY: both pointers come from winit's still-live window. The app
    // calls `shutdown` (joining this thread) before the window is dropped.
    let backend = unsafe { Backend::from_foreign_display(display_ptr.cast()) };
    let conn = Connection::from_backend(backend);

    let setup = (|| {
        let id = unsafe { ObjectId::from_ptr(wl_surface_interface(), surface_ptr.cast()) }.ok()?;
        let target_surface =
            wayland_client::protocol::wl_surface::WlSurface::from_id(&conn, id).ok()?;
        let (globals, queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let seat = globals.bind::<WlSeat, _, _>(&qh, 1..=9, ()).ok()?;
        let manager = globals.bind::<ZwpTabletManagerV2, _, _>(&qh, 1..=1, ()).ok()?;
        let tablet_seat = manager.get_tablet_seat(&seat, &qh, ());
        Some((target_surface, queue, tablet_seat))
    })();
    let _ = ready.send(setup.is_some());
    let Some((target_surface, mut queue, _tablet_seat)) = setup else {
        return;
    };

    let mut state = State { target_surface, samples_tx, ctx, tools: HashMap::new() };

    // Poll with a timeout instead of `blocking_dispatch` so `shutdown` can
    // stop the thread.
    let timeout = Timespec { tv_sec: 0, tv_nsec: 50_000_000 };
    while !stop.load(Ordering::Relaxed) {
        if queue.dispatch_pending(&mut state).is_err() {
            return;
        }
        let _ = conn.flush();
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let fd = guard.connection_fd();
        let mut fds = [PollFd::new(&fd, PollFlags::IN)];
        let readable = matches!(rustix::event::poll(&mut fds, Some(&timeout)), Ok(n) if n > 0);
        if readable && guard.read().is_err() {
            return;
        }
    }
}

fn wl_surface_interface() -> &'static Interface {
    wayland_client::protocol::wl_surface::WlSurface::interface()
}

/// The pen thread's Wayland dispatch state.
struct State {
    /// Only samples over this surface are sent; others belong to other windows.
    target_surface: wayland_client::protocol::wl_surface::WlSurface,
    samples_tx: mpsc::SyncSender<PenSample>,
    ctx: egui::Context,
    /// Per tool, so a pen and its eraser end (or two tablets) never write
    /// into each other's sample.
    tools: HashMap<ObjectId, ToolState>,
}

#[derive(Default)]
struct ToolState {
    focused: bool,
    /// Proximity-out arrived in this frame: send the frame, then unfocus.
    leaving: bool,
    pending: PenSample,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: wayland_client::protocol::wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTabletManagerV2, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpTabletManagerV2,
        _: <ZwpTabletManagerV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTabletV2, ()> for State {
    fn event(
        _: &mut Self,
        tablet: &ZwpTabletV2,
        event: <ZwpTabletV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_tablet_v2::Event::Removed = event {
            tablet.destroy();
        }
    }
}

impl Dispatch<ZwpTabletSeatV2, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwpTabletSeatV2,
        event: zwp_tablet_seat_v2::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        // Tool and tablet proxies are created by event_created_child below.
        let _ = (event, qh);
    }

    wayland_client::event_created_child!(State, ZwpTabletSeatV2, [
        zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE => (ZwpTabletV2, ()),
        zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE => (ZwpTabletToolV2, ()),
    ]);
}

impl Dispatch<ZwpTabletToolV2, ()> for State {
    fn event(
        state: &mut Self,
        tool: &ZwpTabletToolV2,
        event: zwp_tablet_tool_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_tablet_tool_v2::Event;
        if let Event::Removed = event {
            state.tools.remove(&tool.id());
            tool.destroy();
            return;
        }
        // Events accumulate into `pending`; a frame event sends it whole.
        let tool_state = state.tools.entry(tool.id()).or_default();
        match event {
            Event::ProximityIn { surface, .. } => {
                *tool_state = ToolState::default();
                tool_state.focused = surface.id() == state.target_surface.id();
            }
            Event::ProximityOut => {
                tool_state.pending.down = false;
                tool_state.leaving = true;
            }
            Event::Down { .. } => tool_state.pending.down = true,
            Event::Up => tool_state.pending.down = false,
            Event::Motion { x, y } => {
                tool_state.pending.x = x as f32;
                tool_state.pending.y = y as f32;
            }
            Event::Pressure { pressure } => {
                tool_state.pending.pressure = pressure as f32 / 65535.0;
            }
            Event::Tilt { tilt_x, tilt_y } => {
                tool_state.pending.tilt_x = tilt_x as f32;
                tool_state.pending.tilt_y = tilt_y as f32;
            }
            Event::Frame { time } => {
                tool_state.pending.time_ms = time;
                if tool_state.focused && state.samples_tx.try_send(tool_state.pending).is_ok() {
                    state.ctx.request_repaint();
                }
                if tool_state.leaving {
                    tool_state.focused = false;
                    tool_state.leaving = false;
                }
            }
            _ => {}
        }
    }
}
