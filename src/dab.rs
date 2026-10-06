//! Rasterizes plain dabs into the stroke buffer without allocating.
//!
//! The mask math (`opa_at`, `rr_at`, `rr_at_aa`) is ported from
//! hokusai-core's brushmodes.rs (MIT/Apache-2.0), which ports libmypaint:
//! same hardness falloff, same sub-pixel antialiasing below radius 3, same
//! integer blend. What is gone is hokusai's per-dab `Vec` and per-dab
//! environment variable lookup, which cost more than the pixels on small
//! brushes. Dabs that need lock-alpha, colorize, posterize, pigment mode or
//! the eraser setting still go through hokusai's `draw_dab_default`.
//!
//! Short names in the ported helpers (`rr`, `cs`, `sn`, `opa_a`) are
//! libmypaint's, kept so the code can be checked against it line by line.

use hokusai_core::surface::{Dab, TiledSurface};
use hokusai_core::tile::TILE_SIZE;

use crate::compositor::ONE;

/// Whether `draw` can paint this dab, i.e. it uses none of the modes only
/// hokusai implements.
pub fn is_plain(dab: &Dab) -> bool {
    dab.lock_alpha <= 0.0
        && dab.colorize <= 0.0
        && dab.posterize <= 0.0
        && dab.paint <= 0.0
        && dab.alpha_eraser >= 1.0
}

/// Opacity at squared normalized radius `rr`: libmypaint's hardness falloff.
#[inline]
fn opa_at(rr: f32, hardness: f32) -> f32 {
    if rr > 1.0 {
        return 0.0;
    }
    if hardness >= 1.0 {
        return 1.0;
    }
    if rr <= hardness {
        1.0 + rr * (1.0 - 1.0 / hardness)
    } else {
        (hardness / (1.0 - hardness)) * (1.0 - rr)
    }
}

/// Squared normalized distance from pixel (`px`, `py`) to the dab center
/// (`x`, `y`), in the dab's rotated, squashed frame. 1.0 is the edge.
#[allow(clippy::too_many_arguments)]
#[inline]
fn rr_at(px: f32, py: f32, x: f32, y: f32, aspect: f32, cs: f32, sn: f32, inv_r2: f32) -> f32 {
    let yy = py + 0.5 - y;
    let xx = px + 0.5 - x;
    let yyr = (yy * cs - xx * sn) * aspect;
    let xxr = yy * sn + xx * cs;
    (yyr * yyr + xxr * xxr) * inv_r2
}

#[inline]
fn r_sample(x: f32, y: f32, aspect: f32, sn: f32, cs: f32) -> f32 {
    let yyr = (y * cs - x * sn) * aspect;
    let xxr = y * sn + x * cs;
    yyr * yyr + xxr * xxr
}

#[inline]
fn sign_point_in_line(px: f32, py: f32, vx: f32, vy: f32) -> f32 {
    (px - vx) * (-vy) - vx * (py - vy)
}

#[inline]
fn closest_point_to_line(lx: f32, ly: f32, px: f32, py: f32) -> (f32, f32) {
    let l2 = lx * lx + ly * ly;
    let t = (px * lx + py * ly) / l2;
    (lx * t, ly * t)
}

/// `rr_at` with libmypaint's sub-pixel antialiasing, for small radii.
#[allow(clippy::too_many_arguments)]
#[inline]
fn rr_at_aa(
    px: f32,
    py: f32,
    x: f32,
    y: f32,
    aspect: f32,
    cs: f32,
    sn: f32,
    inv_r2: f32,
    r_aa_start: f32,
) -> f32 {
    let pixel_right = x - px;
    let pixel_bottom = y - py;
    let pixel_center_x = pixel_right - 0.5;
    let pixel_center_y = pixel_bottom - 0.5;
    let pixel_left = pixel_right - 1.0;
    let pixel_top = pixel_bottom - 1.0;

    let (nearest_x, nearest_y, rr_near);
    if pixel_left < 0.0 && pixel_right > 0.0 && pixel_top < 0.0 && pixel_bottom > 0.0 {
        nearest_x = 0.0;
        nearest_y = 0.0;
        rr_near = 0.0;
    } else {
        let (nx, ny) = closest_point_to_line(cs, sn, pixel_center_x, pixel_center_y);
        nearest_x = nx.clamp(pixel_left, pixel_right);
        nearest_y = ny.clamp(pixel_top, pixel_bottom);
        rr_near = r_sample(nearest_x, nearest_y, aspect, sn, cs) * inv_r2;
    }
    if rr_near > 1.0 {
        return rr_near;
    }

    let center_sign = sign_point_in_line(pixel_center_x, pixel_center_y, cs, -sn);
    let rad_area_1 = (1.0_f32 / core::f32::consts::PI).sqrt();
    let (farthest_x, farthest_y) = if center_sign < 0.0 {
        (nearest_x - sn * rad_area_1, nearest_y + cs * rad_area_1)
    } else {
        (nearest_x + sn * rad_area_1, nearest_y - cs * rad_area_1)
    };
    let r_far = r_sample(farthest_x, farthest_y, aspect, sn, cs);
    let rr_far = r_far * inv_r2;
    if r_far < r_aa_start {
        return (rr_far + rr_near) * 0.5;
    }
    let visibility_near = 1.0 - rr_near;
    let delta = rr_far - rr_near;
    1.0 - visibility_near / (1.0 + delta)
}

/// Draws one plain dab with libmypaint's normal blend. Returns whether any
/// tile was touched.
pub fn draw<S: TiledSurface + ?Sized>(surface: &mut S, dab: &Dab) -> bool {
    if dab.radius < 0.1 || dab.hardness <= 0.0 || dab.opaque <= 0.0 {
        return false;
    }
    let radius = dab.radius;
    let aspect = dab.aspect_ratio.max(1.0);
    let angle = ((dab.angle / 360.0 * 2.0) as f64 * std::f64::consts::PI) as f32;
    let (sn, cs) = angle.sin_cos();
    let r_aa_start = if radius < 3.0 {
        let start = if radius > 1.0 { radius - 1.0 } else { 0.0 };
        start * start / aspect
    } else {
        -1.0
    };
    let fringe = radius + 1.0;
    let tile_size = TILE_SIZE as i32;
    // Range of tiles the dab's bounding box covers.
    let tx0 = ((dab.x - fringe).floor() as i32).div_euclid(tile_size);
    let ty0 = ((dab.y - fringe).floor() as i32).div_euclid(tile_size);
    let tx1 = ((dab.x + fringe).floor() as i32).div_euclid(tile_size);
    let ty1 = ((dab.y + fringe).floor() as i32).div_euclid(tile_size);

    let color = dab.color;
    let src = [color.r, color.g, color.b].map(|v| (v.clamp(0.0, 1.0) * ONE as f32) as u32);
    let opacity = (dab.opaque.clamp(0.0, 1.0) * ONE as f32) as u32;
    let hardness = dab.hardness.clamp(0.0, 1.0);
    let inv_r2 = 1.0 / (radius * radius);

    let mut painted = false;
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            // Tile origin in document pixels.
            let (ox, oy) = ((tx * tile_size) as f32, (ty * tile_size) as f32);
            // The dab's bounding box always touches its corner tiles; skip
            // the ones the disc itself cannot reach, so no empty buffer
            // tile is allocated and later committed for nothing.
            let nearest_dx = dab.x.clamp(ox, ox + tile_size as f32) - dab.x;
            let nearest_dy = dab.y.clamp(oy, oy + tile_size as f32) - dab.y;
            if nearest_dx * nearest_dx + nearest_dy * nearest_dy > fringe * fringe {
                continue;
            }
            // Dab center, and the pixel range it covers, in tile-local pixels.
            let (xl, yl) = (dab.x - ox, dab.y - oy);
            let lx0 = ((xl - fringe).floor() as i32).max(0);
            let ly0 = ((yl - fringe).floor() as i32).max(0);
            let lx1 = ((xl + fringe).floor() as i32).min(tile_size - 1);
            let ly1 = ((yl + fringe).floor() as i32).min(tile_size - 1);
            if lx0 > lx1 || ly0 > ly1 {
                continue;
            }
            let tile = surface.tile_request_start(tx, ty);
            for yp in ly0..=ly1 {
                let row = &mut tile[yp as usize];
                for xp in lx0..=lx1 {
                    let rr = if r_aa_start >= 0.0 {
                        rr_at_aa(xp as f32, yp as f32, xl, yl, aspect, cs, sn, inv_r2, r_aa_start)
                    } else {
                        rr_at(xp as f32, yp as f32, xl, yl, aspect, cs, sn, inv_r2)
                    };
                    if rr > 1.0 {
                        continue;
                    }
                    let mask = (opa_at(rr, hardness) * ONE as f32) as u32;
                    if mask == 0 {
                        continue;
                    }
                    let opa_a = mask * opacity / ONE;
                    let opa_b = ONE - opa_a;
                    let px = &mut row[xp as usize];
                    px[3] = (opa_a + opa_b * px[3] as u32 / ONE) as u16;
                    px[0] = ((opa_a * src[0] + opa_b * px[0] as u32) / ONE) as u16;
                    px[1] = ((opa_a * src[1] + opa_b * px[1] as u32) / ONE) as u16;
                    px[2] = ((opa_a * src[2] + opa_b * px[2] as u32) / ONE) as u16;
                }
            }
            surface.tile_request_end(tx, ty);
            painted = true;
        }
    }
    painted
}
