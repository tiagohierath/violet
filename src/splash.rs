//! The start-up panel: the photo, what Violet is, and where to go next.
//! Drawn in the app's own type and rows. It closes with its X or by a
//! click outside it, and opens on every launch unless `splash.txt` says off.

use std::process::Command;

use eframe::egui::{
    self, Align2, ColorImage, Context, FontId, Id, Order, Rect, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, pos2, vec2,
};

use crate::theme::Theme;

const PHOTO: &[u8] = include_bytes!("../assets/splash.jpg");
const CHANGELOG: &str = "https://github.com/tiagohierath/violet/releases";
const WEBSITE: &str = "https://navylily.tv";
const LINE: f32 = 24.0;
const MONO: f32 = 14.0;
const WIDTH: f32 = 420.0;

pub struct Splash {
    pub open: bool,
    photo: Option<TextureHandle>,
}

impl Splash {
    pub fn new() -> Self {
        Self { open: enabled(), photo: None }
    }

    pub fn show(&mut self, ctx: &Context, theme: &Theme) {
        if !self.open {
            return;
        }
        if self.photo.is_none() {
            self.photo = decode(ctx);
        }
        let font = FontId::monospace(MONO);
        let screen = ctx.content_rect();

        let backdrop = egui::Area::new(Id::new("splash-backdrop")).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let (_, response) = ui.allocate_exact_size(screen.size(), Sense::click());
            response
        });
        let mut close = backdrop.inner.clicked();

        let start = pos2(screen.center().x - WIDTH / 2.0, screen.center().y - 160.0);
        egui::Area::new(Id::new("splash-panel")).order(Order::Foreground).default_pos(start).movable(true).show(ctx, |ui| {
            ui.set_width(WIDTH);
            let frame = egui::Frame::new()
                .fill(theme.surface)
                .stroke(Stroke::new(1.0, theme.border))
                .inner_margin(8.0);
            frame.show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let bar = ui.allocate_exact_size(vec2(WIDTH - 16.0, LINE), Sense::hover()).0;
                let painter = ui.painter().clone();
                painter.text(bar.left_center(), Align2::LEFT_CENTER, format!("Violet ({})", env!("CARGO_PKG_VERSION")), FontId::monospace(MONO), theme.text);
                let x = Rect::from_center_size(pos2(bar.right() - LINE / 2.0, bar.center().y), vec2(LINE, LINE));
                let x_response = ui.interact(x, Id::new("splash-close"), Sense::click());
                let x_ink = if x_response.hovered() { theme.accent } else { theme.text };
                painter.text(x.center(), Align2::CENTER_CENTER, "x", FontId::monospace(MONO), x_ink);
                close |= x_response.clicked();

                if let Some(photo) = &self.photo {
                    let width = WIDTH - 16.0;
                    let height = width * photo.size()[1] as f32 / photo.size()[0] as f32;
                    ui.add(egui::Image::new((photo.id(), vec2(width, height))));
                }

                for text in ["A drawing app: keyboard first, configured by text files.", ""] {
                    ui.add_sized(vec2(WIDTH - 16.0, LINE), egui::Label::new(RichText::new(text).font(font.clone()).color(theme.text)));
                }
                for (label, url) in [("see changelog", CHANGELOG), ("visit navylily.tv", WEBSITE)] {
                    let (row, response) = ui.allocate_exact_size(vec2(WIDTH - 16.0, LINE), Sense::click());
                    let ink = if response.hovered() { theme.accent } else { theme.text };
                    crate::chrome::outlined_label(&painter, row.left_center(), label, ink);
                    if response.clicked() {
                        open_url(url);
                    }
                }
            });
        });

        if close {
            self.open = false;
        }
    }
}

/// Off only when `splash.txt` says `off`. The file is created as `on` on first launch.
fn enabled() -> bool {
    let path = crate::keymap::config_file("splash.txt");
    if !path.exists() {
        let created = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, "on\n"));
        if let Err(err) = created {
            eprintln!("splash: cannot create {}: {err}", path.display());
        }
    }
    std::fs::read_to_string(&path).map_or(true, |text| text.trim() != "off")
}

fn decode(ctx: &Context) -> Option<TextureHandle> {
    let rgba = image::load_from_memory(PHOTO).ok()?.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let image = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture("splash-photo", image, TextureOptions::LINEAR))
}

fn open_url(url: &str) {
    if let Err(err) = Command::new("xdg-open").arg(url).spawn() {
        eprintln!("splash: cannot open {url}: {err}");
    }
}
