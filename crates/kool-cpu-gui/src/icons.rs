//! Procedural anti-aliased app icon (CPU die + cooling crystal) and
//! Google Sans-inspired geometric tray temperature numerals (2x supersampled).

use std::f32::consts::{FRAC_PI_2, FRAC_PI_3, PI};

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn sd_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let pax = px - ax;
    let pay = py - ay;
    let bax = bx - ax;
    let bay = by - ay;
    let denom = (bax * bax + bay * bay).max(1e-6);
    let h = ((pax * bax + pay * bay) / denom).clamp(0.0, 1.0);
    let dx = pax - bax * h;
    let dy = pay - bay * h;
    (dx * dx + dy * dy).sqrt()
}

fn sd_rounded_box(px: f32, py: f32, bx: f32, by: f32, r: f32) -> f32 {
    let qx = px.abs() - bx + r;
    let qy = py.abs() - by + r;
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

/// Distance to a circular arc centered at (cx, cy) with radius `r` spanning [a0, a1] radians (a1 >= a0).
fn sd_arc(px: f32, py: f32, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> f32 {
    let dx = px - cx;
    let dy = py - cy;
    let mut a = dy.atan2(dx);
    while a < a0 {
        a += 2.0 * PI;
    }
    if a <= a1 {
        ((dx * dx + dy * dy).sqrt() - r).abs()
    } else {
        let d0 = ((px - (cx + r * a0.cos())).powi(2) + (py - (cy + r * a0.sin())).powi(2)).sqrt();
        let d1 = ((px - (cx + r * a1.cos())).powi(2) + (py - (cy + r * a1.sin())).powi(2)).sqrt();
        d0.min(d1)
    }
}

/// Signed distance to a single Google Sans-style geometric glyph in normalized box
/// x in [-0.5, 0.5], y in [-1.0, 1.0] (y increases downward).
fn sd_digit(px: f32, py: f32, digit: usize) -> f32 {
    match digit {
        0 => {
            // Geometric oval/stadium loop
            let qx = px.abs() - 0.12;
            let qy = py.abs() - 0.44;
            let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - 0.34;
            d.abs()
        }
        1 => {
            // Clean stem + short angled top flag (Product/Google Sans style)
            let stem = sd_segment(px, py, 0.06, -0.82, 0.06, 0.82);
            let flag = sd_segment(px, py, 0.06, -0.82, -0.26, -0.56);
            stem.min(flag)
        }
        2 => {
            let top = sd_arc(px, py, 0.0, -0.40, 0.40, -PI * 0.92, 0.25);
            let diag = sd_segment(px, py, 0.38, -0.28, -0.40, 0.80);
            let base = sd_segment(px, py, -0.40, 0.80, 0.42, 0.80);
            top.min(diag).min(base)
        }
        3 => {
            let top = sd_arc(px, py, -0.02, -0.42, 0.38, -PI * 0.85, FRAC_PI_2);
            let bot = sd_arc(px, py, -0.02, 0.38, 0.42, -FRAC_PI_2, PI * 0.85);
            top.min(bot)
        }
        4 => {
            let diag = sd_segment(px, py, 0.20, -0.82, -0.42, 0.28);
            let bar = sd_segment(px, py, -0.42, 0.28, 0.44, 0.28);
            let stem = sd_segment(px, py, 0.20, -0.82, 0.20, 0.82);
            diag.min(bar).min(stem)
        }
        5 => {
            let top = sd_segment(px, py, -0.34, -0.80, 0.36, -0.80);
            let left = sd_segment(px, py, -0.34, -0.80, -0.38, -0.06);
            let bowl = sd_arc(px, py, -0.02, 0.36, 0.44, -PI * 0.72, PI * 0.86);
            top.min(left).min(bowl)
        }
        6 => {
            let bowl = ((px.powi(2) + (py - 0.36).powi(2)).sqrt() - 0.44).abs();
            let diag = sd_segment(px, py, -0.38, 0.16, 0.14, -0.82);
            bowl.min(diag)
        }
        7 => {
            let top = sd_segment(px, py, -0.40, -0.80, 0.42, -0.80);
            let diag = sd_segment(px, py, 0.42, -0.80, -0.14, 0.82);
            top.min(diag)
        }
        8 => {
            let top = ((px.powi(2) + (py + 0.42).powi(2)).sqrt() - 0.37).abs();
            let bot = ((px.powi(2) + (py - 0.38).powi(2)).sqrt() - 0.42).abs();
            top.min(bot)
        }
        9 => {
            let bowl = ((px.powi(2) + (py + 0.36).powi(2)).sqrt() - 0.44).abs();
            let diag = sd_segment(px, py, 0.38, -0.16, -0.14, 0.82);
            bowl.min(diag)
        }
        _ => sd_segment(px, py, -0.30, 0.0, 0.30, 0.0), // '-'
    }
}

fn blend_over(dst: &mut [u8; 4], src: [u8; 3], alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let da = dst[3] as f32 / 255.0;
    let out_a = a + da * (1.0 - a);
    if out_a <= 1e-5 {
        return;
    }
    for i in 0..3 {
        let s = src[i] as f32;
        let d = dst[i] as f32;
        dst[i] = ((s * a + d * da * (1.0 - a)) / out_a).round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// Render a crisp 32x32 RGBA tray icon with Google Sans-inspired geometric numerals
/// using 2x2 supersampling for clean taskbar readability.
pub fn render_temp_tray_rgba(temp_c: Option<i32>) -> Vec<u8> {
    const SIZE: usize = 32;
    let mut out = vec![0u8; SIZE * SIZE * 4];

    let (fg, d0, d1) = match temp_c {
        Some(t) => {
            let c = t.clamp(0, 99) as usize;
            let col = if t >= 80 {
                [248, 113, 113] // coral red
            } else if t >= 70 {
                [251, 191, 36] // warm amber
            } else {
                [110, 231, 183] // cool mint
            };
            (col, c / 10, c % 10)
        }
        None => ([167, 176, 192], 10, 10),
    };

    let bg = [16, 19, 25];
    let border = [48, 56, 70];
    let stroke_r = 0.145; // clean medium-bold geometric stroke

    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut acc = [0.0f32; 4];
            for sy in 0..2 {
                for sx in 0..2 {
                    let fx = x as f32 + (sx as f32 + 0.5) * 0.5;
                    let fy = y as f32 + (sy as f32 + 0.5) * 0.5;
                    let mut px_col = [0u8; 4];

                    // Rounded squircle backdrop
                    let d_box = sd_rounded_box(fx - 16.0, fy - 16.0, 15.2, 15.2, 6.5);
                    let bg_a = 1.0 - smoothstep(-0.6, 0.6, d_box);
                    blend_over(&mut px_col, bg, bg_a * 0.96);

                    // Subtle 1px rim for definition on dark & light taskbars
                    let rim_a = (1.0 - smoothstep(0.0, 1.1, (d_box + 0.6).abs())) * bg_a;
                    blend_over(&mut px_col, border, rim_a * 0.75);

                    // Left & right geometric digits (centered vertically, generous height)
                    let gx0 = (fx - 10.2) / 10.2;
                    let gx1 = (fx - 21.8) / 10.2;
                    let gy = (fy - 16.0) / 10.8;
                    let d_g = sd_digit(gx0, gy, d0).min(sd_digit(gx1, gy, d1)) - stroke_r;
                    let fg_a = 1.0 - smoothstep(-0.045, 0.045, d_g);
                    blend_over(&mut px_col, fg, fg_a);

                    for c in 0..4 {
                        acc[c] += px_col[c] as f32;
                    }
                }
            }
            let idx = (y * SIZE + x) * 4;
            for c in 0..4 {
                out[idx + c] = (acc[c] * 0.25).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Render a unique 64x64 RGBA app icon for kool-cpu:
/// Deep midnight Material squircle + sky-blue CPU die with pins + mint 6-point cooling crystal.
pub fn render_app_icon_rgba(size: usize) -> Vec<u8> {
    let mut out = vec![0u8; size * size * 4];
    let half = size as f32 * 0.5;

    let bg_top = [22u8, 30, 46];
    let bg_bot = [13u8, 17, 24];
    let rim_col = [56u8, 72, 98];
    let cpu_col = [122u8, 184, 255]; // Material sky blue
    let crystal_col = [110u8, 231, 183]; // Ice mint
    let core_col = [232u8, 250, 255]; // Bright frost center

    for y in 0..size {
        for x in 0..size {
            // Normalize to [-1, 1]
            let nx = (x as f32 + 0.5 - half) / half;
            let ny = (y as f32 + 0.5 - half) / half;
            let px_unit = 2.0 / size as f32;
            let mut col = [0u8; 4];

            // Outer squircle
            let d_bg = sd_rounded_box(nx, ny, 0.92, 0.92, 0.26);
            let bg_a = 1.0 - smoothstep(-px_unit, px_unit, d_bg);
            if bg_a > 0.0 {
                let t = ((ny + 1.0) * 0.5).clamp(0.0, 1.0);
                let grad = [
                    (bg_top[0] as f32 * (1.0 - t) + bg_bot[0] as f32 * t) as u8,
                    (bg_top[1] as f32 * (1.0 - t) + bg_bot[1] as f32 * t) as u8,
                    (bg_top[2] as f32 * (1.0 - t) + bg_bot[2] as f32 * t) as u8,
                ];
                blend_over(&mut col, grad, bg_a);

                // Subtle rim
                let rim_a = (1.0 - smoothstep(0.0, px_unit * 1.4, (d_bg + px_unit * 0.8).abs())) * bg_a;
                blend_over(&mut col, rim_col, rim_a * 0.85);

                // CPU package outline + 8 precision pins (2 per side)
                let d_die = sd_rounded_box(nx, ny, 0.48, 0.48, 0.11).abs() - 0.055;
                let mut d_pins = f32::MAX;
                for offset in [-0.20f32, 0.20] {
                    d_pins = d_pins
                        .min(sd_segment(nx, ny, offset, -0.52, offset, -0.68))
                        .min(sd_segment(nx, ny, offset, 0.52, offset, 0.68))
                        .min(sd_segment(nx, ny, -0.52, offset, -0.68, offset))
                        .min(sd_segment(nx, ny, 0.52, offset, 0.68, offset));
                }
                let d_cpu = d_die.min(d_pins - 0.042);
                let cpu_a = 1.0 - smoothstep(-px_unit, px_unit, d_cpu);
                blend_over(&mut col, cpu_col, cpu_a);

                // 6-point cooling crystal inside the CPU die
                let mut d_crys = f32::MAX;
                for i in 0..3 {
                    let a = (i as f32) * FRAC_PI_3 + FRAC_PI_2;
                    let (s, c) = a.sin_cos();
                    let r = 0.27;
                    d_crys = d_crys.min(sd_segment(nx, ny, -c * r, -s * r, c * r, s * r));
                }
                let crys_a = 1.0 - smoothstep(-px_unit, px_unit, d_crys - 0.036);
                blend_over(&mut col, crystal_col, crys_a);

                // Bright frost core dot
                let d_core = (nx * nx + ny * ny).sqrt() - 0.085;
                let core_a = 1.0 - smoothstep(-px_unit, px_unit, d_core);
                blend_over(&mut col, core_col, core_a);
            }

            let idx = (y * size + x) * 4;
            out[idx..idx + 4].copy_from_slice(&col);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_digits_and_app_icon() {
        for t in [0, 12, 34, 56, 78, 99] {
            let buf = render_temp_tray_rgba(Some(t));
            assert_eq!(buf.len(), 32 * 32 * 4);
            assert!(buf.chunks_exact(4).any(|p| p[3] == 255));
        }
        let app = render_app_icon_rgba(64);
        assert_eq!(app.len(), 64 * 64 * 4);
        // Must contain both CPU sky-blue and crystal mint accents
        assert!(app.chunks_exact(4).any(|p| p[2] > 200 && p[3] > 200));
        assert!(app.chunks_exact(4).any(|p| p[1] > 200 && p[3] > 200));
    }
}
