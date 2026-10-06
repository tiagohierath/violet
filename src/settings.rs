//! Dialogs that ask before the drawing on screen is replaced: New (which
//! also asks for the size) and Quit. Each says plainly where the current
//! drawing is saved, so nothing feels deleted. They only push commands.

use std::path::Path;

use eframe::egui::{self, Align, Color32, DragValue, Key, Layout, RichText, Stroke, Ui};

use violet::document::MAX_SIDE;

use crate::commands::{Invocation, Value};
use crate::theme::Theme;

/// Common sizes: print at 300 dpi, portrait; screens, landscape.
pub const PRESETS: &[(&str, u32, u32)] = &[
    ("A5", 1748, 2480),
    ("A4", 2480, 3508),
    ("A3", 3508, 4961),
    ("Letter", 2550, 3300),
    ("HD screen", 1920, 1080),
    ("4K screen", 3840, 2160),
    ("Square", 2048, 2048),
];

pub struct NewDrawing {
    pub width: u32,
    pub height: u32,
    /// Where the current drawing is saved, worked out when the dialog opens.
    pub note: String,
}

/// `drawing-004.lily is saved in ~/Pictures/Violet`.
pub fn saved_in(path: &Path) -> String {
    let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    format!("{name} is saved in {}", folder(path))
}

fn folder(path: &Path) -> String {
    let dir = path.parent().unwrap_or(Path::new("/"));
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match home.as_deref().and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => dir.display().to_string(),
    }
}

/// What happens to the drawing on screen if it is replaced or closed.
/// `target` is where it gets saved: its own path, or the next untitled one.
pub fn save_note(saved: Option<&Path>, modified: bool, target: &Path) -> String {
    match (modified, saved) {
        (true, _) => format!("Your drawing is saved first: {}.", saved_in(target)),
        (false, Some(path)) => format!("Your drawing is safe: {}.", saved_in(path)),
        (false, None) => "Nothing is drawn yet, so there is nothing to save.".into(),
    }
}

fn frame(theme: &Theme) -> egui::Frame {
    egui::Frame::new()
        .fill(theme.surface)
        .stroke(Stroke::new(1.0, theme.border))
        .inner_margin(16)
        .corner_radius(0)
}

/// A dialog box over everything, blocking the UI under it.
fn modal(ctx: &egui::Context, id: &str, theme: &Theme, width: f32, content: impl FnOnce(&mut Ui)) {
    let area = egui::Modal::default_area(egui::Id::new(id)).fade_in(false);
    egui::Modal::new(egui::Id::new(id))
        .area(area)
        .backdrop_color(Color32::TRANSPARENT)
        .frame(frame(theme))
        .show(ctx, |ui| {
            ui.set_width(width);
            content(ui);
        });
}

fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(18.0).strong());
}

/// The primary button: accent fill, so it stands out without gray.
fn primary(ui: &mut Ui, theme: &Theme, text: &str) -> bool {
    let button = egui::Button::new(RichText::new(text).color(theme.on_accent).strong()).fill(theme.accent);
    ui.add(button).clicked()
}

fn buttons(ui: &mut Ui, theme: &Theme, confirm: &str) -> (bool, bool) {
    let mut out = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        out.0 = primary(ui, theme, confirm);
        out.1 = ui.button("Cancel").clicked();
    });
    out
}

pub fn new_drawing(ctx: &egui::Context, d: &mut NewDrawing, theme: &Theme, out: &mut Vec<Invocation>) {
    modal(ctx, "new-drawing", theme, 420.0, |ui| {
        title(ui, "New drawing");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Width");
            ui.add(DragValue::new(&mut d.width).range(1..=MAX_SIDE).suffix(" px"));
            ui.label("Height");
            ui.add(DragValue::new(&mut d.height).range(1..=MAX_SIDE).suffix(" px"));
        });
        ui.horizontal(|ui| {
            let portrait = d.height > d.width;
            let landscape = d.width > d.height;
            let to_portrait = ui.selectable_label(portrait, "Portrait").clicked() && landscape;
            let to_landscape = ui.selectable_label(landscape, "Landscape").clicked() && portrait;
            if to_portrait || to_landscape {
                std::mem::swap(&mut d.width, &mut d.height);
            }
        });
        ui.add_space(8.0);
        ui.label(RichText::new("Sizes").strong());
        egui::Grid::new("presets").num_columns(2).show(ui, |ui| {
            for &(name, w, h) in PRESETS {
                let on = (d.width, d.height) == (w, h) || (d.width, d.height) == (h, w);
                if ui.selectable_label(on, name).clicked() {
                    // Keep the orientation the user picked.
                    (d.width, d.height) = if (d.width > d.height) == (w > h) || w == h { (w, h) } else { (h, w) };
                }
                ui.label(format!("{w} × {h}"));
                ui.end_row();
            }
        });
        ui.add_space(8.0);
        ui.label(RichText::new(&d.note).color(theme.accent));
        ui.add_space(8.0);
        let (create, cancel) = buttons(ui, theme, "Create");
        if create || ui.input(|i| i.key_pressed(Key::Enter)) {
            out.push(Invocation::new(
                "file.new",
                Value::List(vec![Value::Num(d.width.into()), Value::Num(d.height.into())]),
            ));
            out.push(Invocation::new("ui.dismiss", Value::Nil));
        } else if cancel {
            out.push(Invocation::new("ui.dismiss", Value::Nil));
        }
    });
}

pub fn quit(ctx: &egui::Context, note: &str, theme: &Theme, out: &mut Vec<Invocation>) {
    modal(ctx, "quit", theme, 380.0, |ui| {
        title(ui, "Quit Violet?");
        ui.add_space(8.0);
        ui.label(RichText::new(note).color(theme.accent));
        ui.add_space(8.0);
        let (confirm, cancel) = buttons(ui, theme, "Quit");
        if confirm || ui.input(|i| i.key_pressed(Key::Enter)) {
            out.push(Invocation::new("ui.dismiss", Value::Nil));
            out.push(Invocation::new("app.quit", Value::Bool(true)));
        } else if cancel {
            out.push(Invocation::new("ui.dismiss", Value::Nil));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_name_the_file_and_folder() {
        let p = Path::new("/somewhere/art/drawing-004.lily");
        assert_eq!(saved_in(p), "drawing-004.lily is saved in /somewhere/art");
        assert!(save_note(None, true, p).contains("drawing-004.lily is saved in /somewhere/art"));
        assert!(save_note(None, false, p).contains("nothing to save"));
    }
}
