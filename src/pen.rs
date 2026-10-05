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
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::tablet::zv2::client::{
    zwp_tablet_manager_v2::ZwpTabletManagerV2,
    zwp_tablet_seat_v2::{self, ZwpTabletSeatV2},
    zwp_tablet_tool_v2::{self, ZwpTabletToolV2},
    zwp_tablet_v2::{self, ZwpTabletV2},
};

#[derive(Debug, Clone, Copy, Default)]
pub struct PenSample {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub down: bool,
    /// Compositor timestamp of the frame, in milliseconds.
    pub time_ms: u32,
}

pub trait PenSource {
    fn drain(&mut self, out: &mut Vec<PenSample>);
}

pub struct WaylandPenSource {
    rx: mpsc::Receiver<PenSample>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl WaylandPenSource {
    /// Must run before the window (and its `wl_display`) is dropped: the
    /// thread reads from winit's connection and would outlive it otherwise.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl PenSource for WaylandPenSource {
    fn drain(&mut self, out: &mut Vec<PenSample>) {
        out.extend(self.rx.try_iter());
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
        let (tx, rx) = mpsc::sync_channel(1024);
        let (ready_tx, ready_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name("lilyfield-pen".into())
            .spawn(move || {
                pen_thread(display_addr, surface_addr, tx, ctx, ready_tx, thread_stop)
            })
            .ok()?;
        // Setup is synchronous: no tablet manager means no pen source.
        if ready_rx.recv() != Ok(true) {
            let _ = thread.join();
            return None;
        }
        Some(Self { rx, stop, thread: Some(thread) })
    }
}

fn pen_thread(
    display_addr: usize,
    surface_addr: usize,
    tx: mpsc::SyncSender<PenSample>,
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

    let mut state = State {
        target_surface,
        tx,
        ctx,
        tools: HashMap::new(),
    };

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

struct State {
    target_surface: wayland_client::protocol::wl_surface::WlSurface,
    tx: mpsc::SyncSender<PenSample>,
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
        let t = state.tools.entry(tool.id()).or_default();
        match event {
            Event::ProximityIn { surface, .. } => {
                *t = ToolState::default();
                t.focused = surface.id() == state.target_surface.id();
            }
            Event::ProximityOut => {
                t.pending.down = false;
                t.leaving = true;
            }
            Event::Down { .. } => t.pending.down = true,
            Event::Up => t.pending.down = false,
            Event::Motion { x, y } => {
                t.pending.x = x as f32;
                t.pending.y = y as f32;
            }
            Event::Pressure { pressure } => {
                t.pending.pressure = pressure as f32 / 65535.0;
            }
            Event::Tilt { tilt_x, tilt_y } => {
                t.pending.tilt_x = tilt_x as f32;
                t.pending.tilt_y = tilt_y as f32;
            }
            Event::Frame { time } => {
                t.pending.time_ms = time;
                if t.focused && state.tx.try_send(t.pending).is_ok() {
                    state.ctx.request_repaint();
                }
                if t.leaving {
                    t.focused = false;
                    t.leaving = false;
                }
            }
            _ => {}
        }
    }
}
