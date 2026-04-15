//! Benchmark: scalar vs SIMD planar vs SIMD row-gather warp at 1080p.
//!
//! Run with:
//!   cargo run --release --features experimental --example warp_bench
//!
//! Reports Mpix/s for each approach on a single plane and full 3-plane warp.

use std::time::Instant;

use zenfilters::filters::warp_simd;
use zenfilters::filters::{WarpBackground, WarpInterpolation};

fn make_gradient_plane(w: u32, h: u32) -> Vec<f32> {
    let mut plane = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            // Interesting pattern: diagonal gradient with some variation
            let t = (x as f32 + y as f32 * 0.3) / (w as f32 + h as f32 * 0.3);
            plane.push(t * 0.8 + 0.1);
        }
    }
    plane
}

fn rotation_matrix(angle_deg: f32, w: u32, h: u32) -> [f32; 9] {
    let angle_rad = angle_deg * std::f32::consts::PI / 180.0;
    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();
    let cx = (w as f32 - 1.0) * 0.5;
    let cy = (h as f32 - 1.0) * 0.5;
    [
        cos_a,
        sin_a,
        cx - cx * cos_a - cy * sin_a,
        -sin_a,
        cos_a,
        cy + cx * sin_a - cy * cos_a,
        0.0,
        0.0,
        1.0,
    ]
}

/// Scalar reference: use the existing warp code path.
fn scalar_warp_plane(src: &[f32], dst: &mut [f32], w: u32, h: u32, m: &[f32; 9]) {
    let wf = w as f32;
    let hf = h as f32;
    let stride = w as usize;

    for dy in 0..h {
        let dyf = dy as f32;
        let mut sx = m[1] * dyf + m[2];
        let mut sy = m[4] * dyf + m[5];

        for dx in 0..w {
            let out_idx = (dy as usize) * stride + (dx as usize);
            let sx_c = sx.clamp(0.0, wf - 1.0);
            let sy_c = sy.clamp(0.0, hf - 1.0);

            dst[out_idx] = sample_robidoux_scalar(src, stride, w, h, sx_c, sy_c);

            sx += m[0];
            sy += m[3];
        }
    }
}

const B: f64 = 0.37821575509399867;
const C: f64 = 0.31089212245300067;

#[inline]
fn robidoux_scalar(t: f32) -> f32 {
    const A3: f32 = ((12.0 - 9.0 * B - 6.0 * C) / 6.0) as f32;
    const A2: f32 = ((-18.0 + 12.0 * B + 6.0 * C) / 6.0) as f32;
    const A0: f32 = ((6.0 - 2.0 * B) / 6.0) as f32;
    const B3: f32 = ((-B - 6.0 * C) / 6.0) as f32;
    const B2: f32 = ((6.0 * B + 30.0 * C) / 6.0) as f32;
    const B1: f32 = ((-12.0 * B - 48.0 * C) / 6.0) as f32;
    const B0: f32 = ((8.0 * B + 24.0 * C) / 6.0) as f32;

    let t = t.abs();
    if t < 1.0 {
        ((A3 * t + A2) * t) * t + A0
    } else if t < 2.0 {
        (((B3 * t + B2) * t) + B1) * t + B0
    } else {
        0.0
    }
}

fn sample_robidoux_scalar(plane: &[f32], stride: usize, w: u32, h: u32, x: f32, y: f32) -> f32 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let fx = x - ix as f32;
    let fy = y - iy as f32;

    let mut wx = [0.0f32; 4];
    let mut wy = [0.0f32; 4];
    let mut wx_sum = 0.0f32;
    let mut wy_sum = 0.0f32;

    for i in 0..4 {
        let offset = i as i32 - 1;
        let wt_x = robidoux_scalar(offset as f32 - fx);
        let wt_y = robidoux_scalar(offset as f32 - fy);
        wx[i] = wt_x;
        wy[i] = wt_y;
        wx_sum += wt_x;
        wy_sum += wt_y;
    }

    let inv_wx = if wx_sum.abs() > 1e-10 {
        1.0 / wx_sum
    } else {
        1.0
    };
    let inv_wy = if wy_sum.abs() > 1e-10 {
        1.0 / wy_sum
    } else {
        1.0
    };
    for wt in &mut wx {
        *wt *= inv_wx;
    }
    for wt in &mut wy {
        *wt *= inv_wy;
    }

    let mut sum = 0.0f32;
    for j in 0..4 {
        let sy = (iy + j as i32 - 1).clamp(0, h as i32 - 1) as usize;
        let mut row_sum = 0.0f32;
        for i in 0..4 {
            let sx = (ix + i as i32 - 1).clamp(0, w as i32 - 1) as usize;
            row_sum += plane[sy * stride + sx] * wx[i];
        }
        sum += row_sum * wy[j];
    }
    sum
}

fn bench_fn(name: &str, mut f: impl FnMut(), iters: u32, mpix: f64) {
    // Warmup
    for _ in 0..3 {
        f();
    }

    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    let elapsed = start.elapsed();

    let per_iter_ms = elapsed.as_secs_f64() * 1000.0 / iters as f64;
    let mpix_per_s = mpix / (per_iter_ms / 1000.0);

    println!("  {name:30} {per_iter_ms:8.2} ms/iter    {mpix_per_s:8.1} Mpix/s");
}

fn verify_correctness(scalar: &[f32], simd: &[f32], name: &str) {
    let mut max_diff = 0.0f32;
    let mut diff_count = 0u64;
    for (i, (&s, &d)) in scalar.iter().zip(simd.iter()).enumerate() {
        let diff = (s - d).abs();
        if diff > max_diff {
            max_diff = diff;
        }
        if diff > 1e-5 {
            diff_count += 1;
            if diff_count <= 3 {
                eprintln!("  [{name}] diff at {i}: scalar={s:.8}, simd={d:.8}, delta={diff:.8}");
            }
        }
    }
    let total = scalar.len() as f64;
    let pct = diff_count as f64 / total * 100.0;
    println!(
        "  [{name}] max_diff={max_diff:.2e}, diffs>{:.0e}: {diff_count} ({pct:.3}%)",
        1e-5
    );
    assert!(
        max_diff < 1e-3,
        "{name}: max diff {max_diff} exceeds 1e-3 — SIMD output is wrong"
    );
}

fn main() {
    let w = 1920u32;
    let h = 1080u32;
    let n = (w as usize) * (h as usize);
    let mpix = n as f64 / 1_000_000.0;
    let angle_deg = 5.0f32;
    let m = rotation_matrix(angle_deg, w, h);

    println!("=== Warp benchmark: {w}x{h}, {angle_deg} deg rotation, Robidoux 4x4 ===");
    println!("    {mpix:.2} Mpix per iteration\n");

    // Create test data
    let src_l = make_gradient_plane(w, h);
    let src_a = make_gradient_plane(w, h); // different content won't matter for perf
    let src_b = make_gradient_plane(w, h);

    // Allocate output buffers
    let mut dst_scalar = vec![0.0f32; n];
    let mut dst_simd_planar = vec![0.0f32; n];
    let mut dst_simd_rowgather = vec![0.0f32; n];

    // --- Verify correctness first ---
    println!("--- Correctness verification (single plane) ---");

    scalar_warp_plane(&src_l, &mut dst_scalar, w, h, &m);
    warp_simd::warp_plane_simd_planar(
        &src_l,
        &mut dst_simd_planar,
        w,
        h,
        &m,
        WarpBackground::Clamp,
        WarpInterpolation::Robidoux,
    );
    warp_simd::warp_plane_simd_rowgather(
        &src_l,
        &mut dst_simd_rowgather,
        w,
        h,
        &m,
        WarpBackground::Clamp,
        WarpInterpolation::Robidoux,
    );

    verify_correctness(&dst_scalar, &dst_simd_planar, "SIMD planar");
    verify_correctness(&dst_scalar, &dst_simd_rowgather, "SIMD row-gather");
    println!();

    // --- Benchmark single plane ---
    let iters = 10;
    println!("--- Single plane warp ({iters} iterations) ---");

    {
        let src = src_l.clone();
        let mut dst = vec![0.0f32; n];
        bench_fn(
            "Scalar (reference)",
            || {
                scalar_warp_plane(&src, &mut dst, w, h, &m);
            },
            iters,
            mpix,
        );
    }

    {
        let src = src_l.clone();
        let mut dst = vec![0.0f32; n];
        bench_fn(
            "SIMD planar (approach A)",
            || {
                warp_simd::warp_plane_simd_planar(
                    &src,
                    &mut dst,
                    w,
                    h,
                    &m,
                    WarpBackground::Clamp,
                    WarpInterpolation::Robidoux,
                );
            },
            iters,
            mpix,
        );
    }

    {
        let src = src_l.clone();
        let mut dst = vec![0.0f32; n];
        bench_fn(
            "SIMD row-gather (approach B)",
            || {
                warp_simd::warp_plane_simd_rowgather(
                    &src,
                    &mut dst,
                    w,
                    h,
                    &m,
                    WarpBackground::Clamp,
                    WarpInterpolation::Robidoux,
                );
            },
            iters,
            mpix,
        );
    }

    println!();

    // --- Benchmark full 3-plane warp ---
    println!("--- Full 3-plane warp ({iters} iterations) ---");

    {
        let (sl, sa, sb) = (src_l.clone(), src_a.clone(), src_b.clone());
        let (mut dl, mut da, mut db) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
        bench_fn(
            "Scalar 3-plane",
            || {
                scalar_warp_plane(&sl, &mut dl, w, h, &m);
                scalar_warp_plane(&sa, &mut da, w, h, &m);
                scalar_warp_plane(&sb, &mut db, w, h, &m);
            },
            iters,
            mpix * 3.0,
        );
    }

    {
        let (sl, sa, sb) = (src_l.clone(), src_a.clone(), src_b.clone());
        let (mut dl, mut da, mut db) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
        bench_fn(
            "SIMD planar 3-plane (A)",
            || {
                warp_simd::warp_all_planes_simd(
                    &sl,
                    &sa,
                    &sb,
                    None,
                    &mut dl,
                    &mut da,
                    &mut db,
                    None,
                    w,
                    h,
                    &m,
                    WarpBackground::Clamp,
                    WarpInterpolation::Robidoux,
                );
            },
            iters,
            mpix * 3.0,
        );
    }

    {
        let (sl, sa, sb) = (src_l.clone(), src_a.clone(), src_b.clone());
        let (mut dl, mut da, mut db) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
        bench_fn(
            "SIMD row-gather 3-plane (B)",
            || {
                warp_simd::warp_all_planes_rowgather(
                    &sl,
                    &sa,
                    &sb,
                    None,
                    &mut dl,
                    &mut da,
                    &mut db,
                    None,
                    w,
                    h,
                    &m,
                    WarpBackground::Clamp,
                    WarpInterpolation::Robidoux,
                );
            },
            iters,
            mpix * 3.0,
        );
    }

    {
        let (sl, sa, sb) = (src_l.clone(), src_a.clone(), src_b.clone());
        let (mut dl, mut da, mut db) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
        bench_fn(
            "Fused 3-plane scalar (C)",
            || {
                warp_simd::warp_3plane_fused(
                    &sl, &sa, &sb, &mut dl, &mut da, &mut db, w, h, &m, 0.0, 0.0, 0.0, false,
                );
            },
            iters,
            mpix * 3.0,
        );
    }

    {
        let (sl, sa, sb) = (src_l.clone(), src_a.clone(), src_b.clone());
        let (mut dl, mut da, mut db) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
        bench_fn(
            "Fused 3-plane SIMD (D)",
            || {
                warp_simd::warp_3plane_fused_simd(
                    &sl, &sa, &sb, &mut dl, &mut da, &mut db, w, h, &m, 0.0, 0.0, 0.0, false,
                );
            },
            iters,
            mpix * 3.0,
        );
    }

    // Verify correctness of fused approaches
    println!("\n--- Correctness verification (3-plane fused) ---");
    {
        let (sl, sa, sb) = (src_l.clone(), src_a.clone(), src_b.clone());
        let (mut dl_ref, mut da_ref, mut db_ref) =
            (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
        let (mut dl_c, mut da_c, mut db_c) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
        let (mut dl_d, mut da_d, mut db_d) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);

        scalar_warp_plane(&sl, &mut dl_ref, w, h, &m);
        scalar_warp_plane(&sa, &mut da_ref, w, h, &m);
        scalar_warp_plane(&sb, &mut db_ref, w, h, &m);

        warp_simd::warp_3plane_fused(
            &sl, &sa, &sb, &mut dl_c, &mut da_c, &mut db_c, w, h, &m, 0.0, 0.0, 0.0, false,
        );
        warp_simd::warp_3plane_fused_simd(
            &sl, &sa, &sb, &mut dl_d, &mut da_d, &mut db_d, w, h, &m, 0.0, 0.0, 0.0, false,
        );

        verify_correctness(&dl_ref, &dl_c, "Fused scalar L");
        verify_correctness(&dl_ref, &dl_d, "Fused SIMD L");
    }

    println!("\nDone.");
}
