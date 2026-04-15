use zenbench::criterion_compat::*;
use zenbench::{criterion_group, criterion_main};

use zenfilters::filters::{CubeLut, FilmLook, FilmPreset, TensorLut};
use zenfilters::{Filter, FilterContext, OklabPlanes};

// ─── Test data ──────────────────────────────────────────────────────

fn make_oklab_planes(width: u32, height: u32) -> OklabPlanes {
    let n = (width as usize) * (height as usize);
    let mut planes = OklabPlanes::new(width, height);
    for i in 0..n {
        let t = i as f32 / n as f32;
        planes.l[i] = 0.1 + t * 0.8;
        planes.a[i] = (t * core::f32::consts::TAU).sin() * 0.1;
        planes.b[i] = (t * core::f32::consts::TAU).cos() * 0.08;
    }
    planes
}

// ─── Film look generation ───────────────────────────────────────────

fn bench_film_look_generation(c: &mut Criterion) {
    let mut group = c.benchmark_group("film_look_generation");
    group.sample_size(20);

    for &preset in &[
        FilmPreset::Portra,
        FilmPreset::Kodachrome,
        FilmPreset::Velvia,
        FilmPreset::CyberpunkNeon,
        FilmPreset::DesertCrush,
    ] {
        group.bench_function(preset.id(), |b| {
            b.iter(|| black_box(FilmLook::new(black_box(preset))))
        });
    }
    group.finish();
}

// ─── Film look apply (the hot path) ─────────────────────────────────

fn bench_film_look_apply(c: &mut Criterion) {
    let mut group = c.benchmark_group("film_look_apply");

    for &preset in &[
        FilmPreset::Portra,
        FilmPreset::Velvia,
        FilmPreset::Noir,
        FilmPreset::CyberpunkNeon,
        FilmPreset::Blockbuster,
    ] {
        let look = FilmLook::new(preset);
        let base = make_oklab_planes(256, 256);
        group.bench_function(format!("{}_256x256", preset.id()), |b| {
            let mut planes = base.clone();
            let mut ctx = FilterContext::new();
            b.iter(|| {
                // Reset planes each iteration — cheap compared to the filter
                planes.l.copy_from_slice(&base.l);
                planes.a.copy_from_slice(&base.a);
                planes.b.copy_from_slice(&base.b);
                look.apply(black_box(&mut planes), &mut ctx);
            })
        });
    }
    group.finish();
}

// ─── Tensor decomposition ───────────────────────────────────────────

fn bench_tensor_decompose(c: &mut Criterion) {
    let mut group = c.benchmark_group("tensor_decompose");
    group.sample_size(10);

    let lut = CubeLut::identity(17);

    for rank in [5, 8, 12] {
        group.bench_function(format!("17cube_rank{rank}"), |b| {
            b.iter(|| black_box(TensorLut::decompose(black_box(&lut), rank, 25)))
        });
    }
    group.finish();
}

// ─── TensorLut lookup throughput ────────────────────────────────────

fn bench_tensor_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("tensor_lookup");

    let lut = CubeLut::identity(17);
    let tensor = TensorLut::decompose(&lut, 8, 25);

    // 65536 random-ish lookups
    let inputs: Vec<[f32; 3]> = (0..65536)
        .map(|i| {
            let t = i as f32 / 65536.0;
            [
                (t * 7.3).sin().abs(),
                (t * 11.1).sin().abs(),
                (t * 13.7).sin().abs(),
            ]
        })
        .collect();

    group.bench_function("rank8_65k_lookups", |b| {
        b.iter(|| {
            let mut sum = [0.0f32; 3];
            for input in &inputs {
                let out = tensor.lookup(*input);
                sum[0] += out[0];
                sum[1] += out[1];
                sum[2] += out[2];
            }
            black_box(sum)
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_film_look_generation,
    bench_film_look_apply,
    bench_tensor_decompose,
    bench_tensor_lookup,
);
criterion_main!(benches);
