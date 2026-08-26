//! Per-kernel NEON-vs-forced-scalar for zenfilters' SIMD kernels.
//!
//! `color_grading.rs` and `row_batch.rs` measure whole filter pipelines. An
//! aggregate cannot reveal a single kernel SLOWER than its own scalar
//! fallback — the faster kernels average it away. That failure mode was found
//! and fixed in garb, zensim, zentone, zenpng and zenresize during the
//! 2026-07-28 aarch64 sweep, so these 11 kernels are checked individually.
//!
//! NOTE: on aarch64 NEON is BASELINE, so the "scalar" arm is the magetypes
//! scalar tier WITH LLVM autovectorization. A ratio near 1.00 does NOT mean a
//! kernel is missing — it means both arms compiled to equivalent work. What a
//! ratio BELOW 1.00 means is that the hand-written kernel is losing to the
//! autovectorizer, which is the thing worth finding.
//!
//! Run: `cargo bench --bench kernel_tiers`
//! Do NOT pass `-C target-cpu=native`: that pins the tier at compile time.

use zenbench::prelude::*;
use zenfilters::__bench_kernels as k;
use zenpixels_convert::gamut::GamutMatrix;

#[cfg(target_arch = "aarch64")]
type TierToken = archmage::NeonToken;
#[cfg(target_arch = "x86_64")]
type TierToken = archmage::X64V3Token;

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
const TIER_NAME: &str = if cfg!(target_arch = "aarch64") {
    "neon"
} else {
    "v3(avx2)"
};

#[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
fn set_simd(enabled: bool) -> bool {
    TierToken::dangerously_disable_token_process_wide(!enabled).is_ok()
}
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn set_simd(_enabled: bool) -> bool {
    false
}

fn planef(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / 16_777_216.0
        })
        .collect()
}

fn planeu(n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 24) as u8
        })
        .collect()
}

/// One 1 MP plane — the realistic granularity for a planar Oklab filter pass.
const N: usize = 1024 * 1024;

macro_rules! two_arms {
    ($suite:expr, $name:expr, $setup:expr) => {{
        $suite.compare($name, |g| {
            g.throughput(Throughput::Elements(N as u64));
            for (arm, simd) in [(TIER_NAME, true), ("scalar", false)] {
                g.bench(arm, move |b| ($setup)(b, simd));
            }
        });
    }};
}

fn bench_kernels(suite: &mut Suite) {
    if !set_simd(true) || !set_simd(false) {
        eprintln!("[kernel_tiers] SIMD tier not toggleable here. Skipping.");
        return;
    }
    set_simd(true);
    eprintln!("[kernel_tiers] comparing {TIER_NAME} vs forced scalar");

    two_arms!(suite, "scale_plane", |b: &mut Bencher, simd| {
        let mut p = planef(N, 3);
        b.iter(move || {
            set_simd(simd);
            k::scale_plane(&mut p, 1.05)
        })
    });
    two_arms!(suite, "power_contrast_plane", |b: &mut Bencher, simd| {
        let mut p = planef(N, 5);
        b.iter(move || {
            set_simd(simd);
            k::power_contrast_plane(&mut p, 1.2, 1.0)
        })
    });
    two_arms!(suite, "sigmoid_tone_map_plane", |b: &mut Bencher, simd| {
        let mut p = planef(N, 7);
        b.iter(move || {
            set_simd(simd);
            k::sigmoid_tone_map_plane(&mut p, 1.3, 0.5)
        })
    });
    two_arms!(suite, "highlights_shadows", |b: &mut Bencher, simd| {
        let mut p = planef(N, 11);
        b.iter(move || {
            set_simd(simd);
            k::highlights_shadows(&mut p, 0.3, -0.2)
        })
    });
    two_arms!(suite, "hue_rotate", |b: &mut Bencher, simd| {
        let (mut a, mut c) = (planef(N, 13), planef(N, 17));
        b.iter(move || {
            set_simd(simd);
            k::hue_rotate(&mut a, &mut c, 0.87, 0.49)
        })
    });
    two_arms!(suite, "vibrance", |b: &mut Bencher, simd| {
        let (mut a, mut c) = (planef(N, 19), planef(N, 23));
        b.iter(move || {
            set_simd(simd);
            k::vibrance(&mut a, &mut c, 0.4, 0.5)
        })
    });
    two_arms!(suite, "unsharp_fuse", |b: &mut Bencher, simd| {
        let (s, bl) = (planef(N, 29), planef(N, 31));
        let mut d = planef(N, 37);
        b.iter(move || {
            set_simd(simd);
            k::unsharp_fuse(&s, &bl, &mut d, 0.7)
        })
    });
    two_arms!(suite, "square_plane", |b: &mut Bencher, simd| {
        let s = planef(N, 41);
        let mut d = planef(N, 43);
        b.iter(move || {
            set_simd(simd);
            k::square_plane(&s, &mut d)
        })
    });
    two_arms!(suite, "subtract_planes", |b: &mut Bencher, simd| {
        let (x, y) = (planef(N, 47), planef(N, 53));
        let mut d = planef(N, 59);
        b.iter(move || {
            set_simd(simd);
            k::subtract_planes(&x, &y, &mut d)
        })
    });

    two_arms!(suite, "brilliance_apply", |b: &mut Bencher, simd| {
        let (src, avg) = (planef(N, 79), planef(N, 83));
        let mut dst = planef(N, 89);
        b.iter(move || {
            set_simd(simd);
            k::brilliance_apply(&src, &avg, &mut dst, 0.5, 0.4, 0.3)
        })
    });
    two_arms!(suite, "adaptive_sharpen_apply", |b: &mut Bencher, simd| {
        let (l, d, e) = (planef(N, 97), planef(N, 101), planef(N, 103));
        let mut dst = planef(N, 107);
        b.iter(move || {
            set_simd(simd);
            k::adaptive_sharpen_apply(&l, &d, &e, &mut dst, 0.6, 0.02, 0.1)
        })
    });

    // The fused colour-space entry/exit points: u8 <-> planar Oklab.
    let m1 = GamutMatrix::default();
    two_arms!(suite, "scatter_srgb_u8_to_oklab", move |b: &mut Bencher, simd| {
        let src = planeu(N * 3, 61);
        let (mut l, mut a, mut c) = (vec![0f32; N], vec![0f32; N], vec![0f32; N]);
        let m = m1;
        b.iter(move || {
            set_simd(simd);
            k::scatter_srgb_u8_to_oklab(&src, &mut l, &mut a, &mut c, 3, &m)
        })
    });
    two_arms!(suite, "gather_oklab_to_srgb_u8", move |b: &mut Bencher, simd| {
        let (l, a, c) = (planef(N, 67), planef(N, 71), planef(N, 73));
        let mut dst = vec![0u8; N * 3];
        let m = m1;
        b.iter(move || {
            set_simd(simd);
            k::gather_oklab_to_srgb_u8(&l, &a, &c, &mut dst, 3, &m)
        })
    });

    set_simd(true);
}


/// The scale+offset fusion, against the two-call sequence it replaces.
/// Three filters ran `scale_plane` then `offset_plane` on the SAME plane —
/// two full read+write passes for arithmetic that fits in one.
fn bench_scale_offset_fusion(suite: &mut Suite) {
    const N: usize = 1 << 20;
    suite.compare("scale+offset+clamp plane", |g| {
        g.throughput(Throughput::Bytes((N * 4) as u64));
        g.bench("fused", |b| {
            b.with_input(|| planef(N, 93))
                .run(move |mut p| { k::scale_offset_clamp_plane(&mut p, 1.3, -0.15, 0.0, 1.0); p })
        });
        g.bench("sequence", |b| {
            b.with_input(|| planef(N, 93)).run(move |mut p| {
                k::scale_plane(&mut p, 1.3);
                k::offset_plane(&mut p, -0.15);
                for v in p.iter_mut() { *v = v.clamp(0.0, 1.0); }
                p
            })
        });
    });
    suite.compare("scale+offset plane", |g| {
        g.throughput(Throughput::Bytes((N * 4) as u64));
        g.bench("fused", |b| {
            b.with_input(|| planef(N, 91))
                .run(move |mut p| { k::scale_offset_plane(&mut p, 1.3, -0.15); p })
        });
        g.bench("sequence", |b| {
            b.with_input(|| planef(N, 91))
                .run(move |mut p| { k::scale_plane(&mut p, 1.3); k::offset_plane(&mut p, -0.15); p })
        });
    });
}

zenbench::main!(bench_scale_offset_fusion, bench_kernels);
