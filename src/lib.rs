//! Lilyfield's core: the document and its pixels, brush strokes, history,
//! files and measurement. No window and no egui here, so all of it runs
//! headless: in tests, in the benchmark, and later under Lua.

pub mod brushes;
pub mod compositor;
pub mod dab;
pub mod document;
pub mod history;
pub mod perf;
pub mod stroke;
