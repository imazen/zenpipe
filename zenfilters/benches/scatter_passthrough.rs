//! sRGB-passthrough scatter: the per-tier (`autoversion`) deinterleave against
//! the plain index loop it replaced, compiled at the crate's baseline target.
//!
//! Sizes cover a tiny strip, the L2/L3-resident strips `Pipeline::apply_*`
//! actually passes (strip height clamps the working set to about 4 MB, so
//! 250k-350k pixels per call), and a full megapixel.
//!
//! Run: `cargo bench -p zenfilters --bench scatter_passthrough`
//! Do NOT pass `-C target-cpu=native`: it would give the baseline loop AVX2 too.

use zenbench::prelude::*;
use zenfilters::__bench_kernels as k;

const SIZES: [(&str, usize); 4] = [
    ("4k", 4_096),
    ("64k", 65_536),
    ("349k", 349_440),
    ("1M", 1_048_576),
];

fn src(n: usize) -> Vec<f32> {
    let mut s = 0x9e37_79b9u32;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (s >> 8) as f32 / 16_777_216.0
        })
        .collect()
}

#[inline(never)]
fn index_loop3(src: &[f32], r: &mut [f32], g: &mut [f32], b: &mut [f32]) {
    for i in 0..r.len() {
        r[i] = src[i * 3];
        g[i] = src[i * 3 + 1];
        b[i] = src[i * 3 + 2];
    }
}

#[inline(never)]
fn index_loop4(src: &[f32], r: &mut [f32], g: &mut [f32], b: &mut [f32], a: &mut [f32]) {
    for i in 0..r.len() {
        r[i] = src[i * 4];
        g[i] = src[i * 4 + 1];
        b[i] = src[i * 4 + 2];
        a[i] = src[i * 4 + 3];
    }
}

fn bench_scatter(suite: &mut Suite) {
    for (label, n) in SIZES {
        suite.compare(format!("rgb {label}"), move |g| {
            g.throughput(Throughput::Elements(n as u64));
            g.bench("index loop", move |b| {
                let s = src(n * 3);
                let (mut r, mut gg, mut bb) = (vec![0f32; n], vec![0f32; n], vec![0f32; n]);
                b.iter(move || index_loop3(&s, &mut r, &mut gg, &mut bb))
            });
            g.bench("autoversion", move |b| {
                let s = src(n * 3);
                let (mut r, mut gg, mut bb) = (vec![0f32; n], vec![0f32; n], vec![0f32; n]);
                b.iter(move || k::deinterleave3(&s, &mut r, &mut gg, &mut bb))
            });
        });
        suite.compare(format!("rgba {label}"), move |g| {
            g.throughput(Throughput::Elements(n as u64));
            g.bench("index loop", move |b| {
                let s = src(n * 4);
                let mut p = [vec![0f32; n], vec![0f32; n], vec![0f32; n], vec![0f32; n]];
                b.iter(move || {
                    let [r, gg, bb, a] = &mut p;
                    index_loop4(&s, r, gg, bb, a)
                })
            });
            g.bench("autoversion", move |b| {
                let s = src(n * 4);
                let mut p = [vec![0f32; n], vec![0f32; n], vec![0f32; n], vec![0f32; n]];
                b.iter(move || {
                    let [r, gg, bb, a] = &mut p;
                    k::deinterleave4(&s, r, gg, bb, a)
                })
            });
        });
    }
}

zenbench::main!(bench_scatter);
