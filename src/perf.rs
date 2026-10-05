//! Measurement that ships with the app, not a profiler session: every phase
//! of a frame is timed, allocations are counted, and the numbers are always
//! on screen one command away. Lilyfield must run on any laptop, so a
//! regression has to be visible the day it lands.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

/// Counts every heap allocation in the process. Each binary installs it with
/// `#[global_allocator]`; the cost is two relaxed atomic adds per allocation.
pub struct CountingAlloc;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(new_size as u64, Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// `(allocations, bytes)` since process start.
pub fn alloc_totals() -> (u64, u64) {
    (ALLOCS.load(Relaxed), ALLOC_BYTES.load(Relaxed))
}

/// The timed phases. A closed set, so the overlay and the bench always
/// report the same columns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Span {
    /// Turning raw mouse and pen events into commands.
    Input,
    /// hokusai dab generation plus rasterizing dabs into the stroke buffer.
    Stroke,
    /// Compositing a finished stroke into the document and recording undo.
    Commit,
    /// Undo or redo, including the display invalidation it causes.
    Undo,
    /// Flattening dirty tiles into display pixels.
    Composite,
    /// Handing display pixels to egui's texture manager.
    Upload,
    /// Building the canvas meshes.
    Paint,
    /// Status line, popups and every other element.
    Ui,
    /// All of Lilyfield's own work in one frame.
    Frame,
    Save,
    Load,
    Export,
}

impl Span {
    pub const ALL: [Span; 12] = [
        Span::Input,
        Span::Stroke,
        Span::Commit,
        Span::Undo,
        Span::Composite,
        Span::Upload,
        Span::Paint,
        Span::Ui,
        Span::Frame,
        Span::Save,
        Span::Load,
        Span::Export,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Span::Input => "input",
            Span::Stroke => "stroke",
            Span::Commit => "commit",
            Span::Undo => "undo/redo",
            Span::Composite => "composite",
            Span::Upload => "upload",
            Span::Paint => "paint",
            Span::Ui => "ui",
            Span::Frame => "frame",
            Span::Save => "save",
            Span::Load => "load",
            Span::Export => "export",
        }
    }
}

/// Last, smoothed and peak duration of one span, in milliseconds.
#[derive(Clone, Copy, Default, Debug)]
pub struct Stat {
    pub last: f32,
    pub avg: f32,
    pub peak: f32,
    pub count: u64,
}

impl Stat {
    pub fn record(&mut self, ms: f32) {
        self.last = ms;
        self.count += 1;
        self.avg = if self.count == 1 { ms } else { self.avg + (ms - self.avg) * 0.05 };
        // The peak decays so it shows recent spikes, not one from an hour ago.
        self.peak = ms.max(self.peak * 0.99);
    }
}

/// Per-frame counters. Collected while the frame runs, then frozen into
/// `Perf::last` when it ends.
#[derive(Clone, Copy, Default, Debug)]
pub struct Counters {
    pub allocs: u64,
    pub alloc_bytes: u64,
    pub samples: u64,
    pub dabs: u64,
    pub tiles_composited: u64,
    pub uploads: u64,
    pub upload_bytes: u64,
    pub draw_calls: u64,
}

pub struct Perf {
    stats: [Stat; Span::ALL.len()],
    pub frame: Counters,
    pub last: Counters,
    /// Wall time between consecutive frames, which is what the eye sees.
    pub interval: Stat,
    /// eframe's own CPU time for the previous frame (tessellation and GPU
    /// submission included), when the backend reports it.
    pub eframe_cpu: Option<f32>,
    frame_start: Option<Instant>,
    alloc_mark: (u64, u64),
}

impl Default for Perf {
    fn default() -> Self {
        Self::new()
    }
}

impl Perf {
    pub fn new() -> Self {
        Self {
            stats: [Stat::default(); Span::ALL.len()],
            frame: Counters::default(),
            last: Counters::default(),
            interval: Stat::default(),
            eframe_cpu: None,
            frame_start: None,
            alloc_mark: alloc_totals(),
        }
    }

    pub fn stat(&self, span: Span) -> &Stat {
        &self.stats[span as usize]
    }

    pub fn record(&mut self, span: Span, ms: f32) {
        self.stats[span as usize].record(ms);
    }

    pub fn record_since(&mut self, span: Span, start: Instant) {
        self.record(span, ms_since(start));
    }

    /// Starts a frame. Allocations are counted from one frame start to the
    /// next, so they include egui's tessellation and the GPU submission too.
    pub fn begin_frame(&mut self) {
        let now = Instant::now();
        if let Some(prev) = self.frame_start {
            self.interval.record((now - prev).as_secs_f32() * 1000.0);
            let (a, b) = alloc_totals();
            self.frame.allocs = a - self.alloc_mark.0;
            self.frame.alloc_bytes = b - self.alloc_mark.1;
            self.alloc_mark = (a, b);
            self.last = self.frame;
            self.frame = Counters::default();
        }
        self.frame_start = Some(now);
    }

    pub fn end_frame(&mut self) {
        if let Some(start) = self.frame_start {
            self.record_since(Span::Frame, start);
        }
    }
}

pub fn ms_since(start: Instant) -> f32 {
    start.elapsed().as_secs_f32() * 1000.0
}
