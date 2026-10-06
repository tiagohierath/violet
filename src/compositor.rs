//! All channel math lives here (ARCHITECTURE §2). Pixels are fix15
//! (`1 << 15` is 1.0), premultiplied alpha, linear light: exactly hokusai's
//! tile format, so dabs land in document tiles with no conversion. A later
//! pigment mode replaces this module rather than a search across the code.

use std::sync::{Arc, OnceLock};

use hokusai_core::tile::{TILE_SIZE, TilePixels};

/// 1.0 in fix15.
pub const ONE: u32 = 1 << 15;
/// Tile side in pixels.
pub const N: usize = TILE_SIZE;
/// Pixels in one tile.
pub const TILE_PX: usize = N * N;

/// One pixel: premultiplied linear RGBA, fix15.
pub type Px = [u16; 4];

/// fix15 multiply with libmypaint's rounding.
#[inline]
pub fn mul(a: u32, b: u32) -> u32 {
    (a * b + (1 << 14)) >> 15
}

/// A fully transparent tile.
pub fn new_tile() -> Box<TilePixels> {
    // SAFETY: an all-zero bit pattern is a valid array of u16.
    unsafe { Box::<TilePixels>::new_zeroed().assume_init() }
}

pub fn new_shared_tile() -> Arc<TilePixels> {
    // SAFETY: as above. Allocating the Arc directly avoids building the
    // 32 KB tile in a Box and copying it in.
    unsafe { Arc::<TilePixels>::new_zeroed().assume_init() }
}

/// A tile's rows as one flat slice, row-major.
#[inline]
pub fn pixels(tile: &TilePixels) -> &[Px] {
    tile.as_flattened()
}

/// How a stroke's buffer lands on its layer: paint goes over, the eraser
/// takes away. Same buffer, same dabs, one flag (ARCHITECTURE §4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StrokeBlend {
    Normal,
    Erase,
}

/// One pixel of a stroke buffer composited onto its layer at the stroke's
/// opacity. The buffer holds the whole stroke at full strength, so this
/// runs once per pixel per stroke and overlapping dabs never double-darken.
#[inline]
pub fn apply_stroke(dst: Px, src: Px, opacity: u32, blend: StrokeBlend) -> Px {
    let src_alpha = mul(src[3] as u32, opacity);
    if src_alpha == 0 {
        return dst;
    }
    let keep = ONE - src_alpha;
    match blend {
        StrokeBlend::Normal => [
            (mul(src[0] as u32, opacity) + mul(dst[0] as u32, keep)).min(ONE) as u16,
            (mul(src[1] as u32, opacity) + mul(dst[1] as u32, keep)).min(ONE) as u16,
            (mul(src[2] as u32, opacity) + mul(dst[2] as u32, keep)).min(ONE) as u16,
            (src_alpha + mul(dst[3] as u32, keep)).min(ONE) as u16,
        ],
        StrokeBlend::Erase => dst.map(|c| mul(c as u32, keep) as u16),
    }
}

/// Source-over for premultiplied pixels.
#[inline]
pub fn over(src: Px, dst: Px) -> Px {
    let keep = ONE - src[3] as u32;
    [
        (src[0] as u32 + mul(dst[0] as u32, keep)).min(ONE) as u16,
        (src[1] as u32 + mul(dst[1] as u32, keep)).min(ONE) as u16,
        (src[2] as u32 + mul(dst[2] as u32, keep)).min(ONE) as u16,
        (src[3] as u32 + mul(dst[3] as u32, keep)).min(ONE) as u16,
    ]
}

/// Every channel times a fix15 factor; a premultiplied pixel at `factor`
/// opacity.
#[inline]
pub fn scale(p: Px, factor: u32) -> Px {
    p.map(|c| mul(c as u32, factor) as u16)
}

/// The tile left behind when a finished stroke is composited onto `base`.
/// Pixels outside `valid` (the part of the tile inside the document) are
/// cleared: paint outside a fixed-size document does not exist. `None`
/// means the result is fully transparent, which keeps layers sparse.
pub fn commit_tile(
    base: Option<&TilePixels>,
    scratch: &TilePixels,
    opacity: u32,
    blend: StrokeBlend,
    valid: (usize, usize),
) -> Option<Arc<TilePixels>> {
    let mut out = new_shared_tile();
    let out_px = Arc::get_mut(&mut out).expect("freshly allocated tile is unique");
    // OR of every alpha: nonzero iff any pixel has coverage.
    let mut alpha = 0u16;
    for y in 0..valid.1 {
        for x in 0..valid.0 {
            let under = base.map_or([0; 4], |b| b[y][x]);
            let px = apply_stroke(under, scratch[y][x], opacity, blend);
            alpha |= px[3];
            out_px[y][x] = px;
        }
    }
    (alpha != 0).then_some(out)
}

/// One layer's contribution to a flattened tile.
#[derive(Clone, Copy)]
pub struct LayerTile<'a> {
    pub tile: Option<&'a TilePixels>,
    /// The live stroke, when it is being painted on this layer.
    pub stroke: Option<(&'a TilePixels, u32, StrokeBlend)>,
    pub opacity: u32,
}

/// Flattens layers, bottom first, into `out`. Returns false when no layer
/// has anything here, in which case `out` is untouched and the caller can
/// fill plain paper without converting a single pixel.
pub fn flatten<'a>(out: &mut [Px], layers: impl IntoIterator<Item = LayerTile<'a>>) -> bool {
    let out = &mut out[..TILE_PX];
    let mut first = true;
    for layer in layers {
        if layer.tile.is_none() && layer.stroke.is_none() {
            continue;
        }
        let tile = layer.tile.map(pixels);
        let stroke = layer.stroke.map(|(s, op, blend)| (pixels(s), op, blend));
        for i in 0..TILE_PX {
            let mut p = match tile {
                Some(t) => t[i],
                None => [0; 4],
            };
            if let Some((stroke_px, op, blend)) = stroke {
                p = apply_stroke(p, stroke_px[i], op, blend);
            }
            if layer.opacity < ONE {
                p = scale(p, layer.opacity);
            }
            // The first layer with content is copied, not composited, so
            // `out` never needs clearing.
            out[i] = if first { p } else { over(p, out[i]) };
        }
        first = false;
    }
    !first
}

/// Flattened value of one pixel; `layers` yields `(pixel, layer opacity)`
/// bottom first.
pub fn flatten_px(layers: impl IntoIterator<Item = (Px, u32)>) -> Px {
    layers.into_iter().fold([0; 4], |acc, (p, op)| over(scale(p, op), acc))
}

/// Premultiplied linear pixels to opaque sRGB over the paper, for display
/// and for flattened exports. `put` receives `(index, rgb)`.
#[inline]
pub fn to_display(src: &[Px], paper: [u16; 3], mut put: impl FnMut(usize, [u8; 3])) {
    let lut = lut_srgb8();
    for (i, p) in src.iter().enumerate() {
        let keep = ONE - p[3] as u32;
        let channel = |c: usize| lut[(p[c] as u32 + mul(paper[c] as u32, keep)).min(ONE) as usize];
        put(i, [channel(0), channel(1), channel(2)]);
    }
}

/// The paper's own display color.
pub fn paper_rgb(paper: [u16; 3]) -> [u8; 3] {
    let lut = lut_srgb8();
    paper.map(|c| lut[c as usize])
}

/// 2x box filter in premultiplied linear light, which is the only space
/// where averaging is correct. `src` is `n` by `n`.
pub fn downsample(src: &[Px], n: usize, dst: &mut [Px]) {
    let half = n / 2;
    for y in 0..half {
        let row0 = &src[2 * y * n..2 * y * n + n];
        let row1 = &src[(2 * y + 1) * n..(2 * y + 1) * n + n];
        for x in 0..half {
            // The 2x2 block: a b on the top row, c d below.
            let (a, b, c, d) = (row0[2 * x], row0[2 * x + 1], row1[2 * x], row1[2 * x + 1]);
            dst[y * half + x] = [0, 1, 2, 3].map(|k| {
                let sum = a[k] as u32 + b[k] as u32 + c[k] as u32 + d[k] as u32;
                ((sum + 2) >> 2) as u16
            });
        }
    }
}

pub fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Linear fix15 (0..=32768) to sRGB u8.
fn lut_srgb8() -> &'static [u8] {
    static LUT: OnceLock<Box<[u8]>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..=ONE).map(|i| (linear_to_srgb(i as f64 / ONE as f64) * 255.0).round() as u8).collect()
    })
}

/// Linear fix15 to sRGB u16, for 16-bit file layers.
fn lut_srgb16() -> &'static [u16] {
    static LUT: OnceLock<Box<[u16]>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..=ONE)
            .map(|i| (linear_to_srgb(i as f64 / ONE as f64) * 65535.0).round() as u16)
            .collect()
    })
}

/// sRGB u16 to linear fix15.
fn lut_linear() -> &'static [u16] {
    static LUT: OnceLock<Box<[u16]>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..=u16::MAX as u32)
            .map(|i| (srgb_to_linear(i as f64 / 65535.0) * ONE as f64).round() as u16)
            .collect()
    })
}

pub fn srgb8_to_fix15(c: u8) -> u16 {
    // 257 widens u8 to u16 exactly (255 * 257 = 65535).
    lut_linear()[c as usize * 257]
}

/// Premultiplied linear fix15 to straight sRGB 16-bit, the encoding of an
/// OpenRaster layer PNG. Round-trips through `from_file_px` to within one
/// fix15 step near white and exactly elsewhere.
pub fn to_file_px(p: Px) -> [u16; 4] {
    let lut = lut_srgb16();
    let alpha = p[3] as u32;
    if alpha == 0 {
        return [0; 4];
    }
    let straight = |c: u16| lut[((c as u32 * ONE + alpha / 2) / alpha).min(ONE) as usize];
    let alpha16 = ((alpha * 65535 + ONE / 2) / ONE) as u16;
    [straight(p[0]), straight(p[1]), straight(p[2]), alpha16]
}

/// The inverse of `to_file_px`.
pub fn from_file_px(p: [u16; 4]) -> Px {
    let lut = lut_linear();
    let alpha = (p[3] as u32 * ONE + 32767) / 65535;
    if alpha == 0 {
        return [0; 4];
    }
    let premultiply = |c: u16| mul(lut[c as usize] as u32, alpha).min(alpha) as u16;
    [premultiply(p[0]), premultiply(p[1]), premultiply(p[2]), alpha as u16]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_opacity_stroke_replaces() {
        let red = [ONE as u16, 0, 0, ONE as u16];
        let blue = [0, 0, ONE as u16, ONE as u16];
        assert_eq!(apply_stroke(blue, red, ONE, StrokeBlend::Normal), red);
        assert_eq!(apply_stroke(blue, red, ONE, StrokeBlend::Erase), [0; 4]);
    }

    #[test]
    fn half_opacity_erase_halves() {
        let blue = [0, 0, ONE as u16, ONE as u16];
        let mask = [0, 0, 0, ONE as u16];
        let out = apply_stroke(blue, mask, ONE / 2, StrokeBlend::Erase);
        assert_eq!(out[3], ONE as u16 / 2);
    }

    #[test]
    fn file_pixels_round_trip() {
        for a in [1u16, 7, 300, 16384, 32768] {
            for c in [0u16, 1, a / 3, a / 2, a] {
                let p = [c, a / 2, a, a];
                let back = from_file_px(to_file_px(p));
                for k in 0..4 {
                    assert!((back[k] as i32 - p[k] as i32).abs() <= 1, "{p:?} -> {back:?}");
                }
            }
        }
    }

    #[test]
    fn white_paper_is_white() {
        assert_eq!(paper_rgb([ONE as u16; 3]), [255; 3]);
        assert_eq!(srgb8_to_fix15(255), ONE as u16);
        assert_eq!(srgb8_to_fix15(0), 0);
    }
}
