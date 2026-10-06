mod app;
mod chrome;
mod commands;
mod display;
mod input;
mod io;
mod keymap;
mod layout;
mod pen;
mod settings;
mod splash;
mod theme;
mod view;

#[global_allocator]
static ALLOCATOR: violet::perf::CountingAlloc = violet::perf::CountingAlloc;

fn main() -> eframe::Result<()> {
    let started = std::time::Instant::now();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Violet")
            .with_app_id("violet")
            .with_inner_size([1280.0, 840.0])
            .with_min_inner_size([640.0, 480.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "Violet",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, started)))),
    )
}
