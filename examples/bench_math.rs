//! Microbenchmark: std math vs fast_math scalar approximations.
//!
//! Run: cargo run --release --example bench_math

use std::hint::black_box;
use std::time::Instant;

#[inline]
fn fast_atan2(y: f32, x: f32) -> f32 {
    let ax = x.abs();
    let ay = y.abs();
    let mn = ax.min(ay);
    let mx = ax.max(ay);
    if mx < 1e-20 {
        return 0.0;
    }
    let a = mn / mx;
    let s = a * a;
    let mut r = ((-0.046_496_473 * s + 0.15931422) * s - 0.327_622_77) * s * a + a;
    if ay > ax {
        r = core::f32::consts::FRAC_PI_2 - r;
    }
    if x < 0.0 {
        r = core::f32::consts::PI - r;
    }
    if y < 0.0 {
        r = -r;
    }
    r
}

#[inline]
fn fast_sincos(x: f32) -> (f32, f32) {
    use core::f32::consts::{FRAC_PI_2, PI, TAU};
    let reduced = x - (x * (1.0 / TAU)).round() * TAU;
    let (r, cos_sign) = if reduced > FRAC_PI_2 {
        (PI - reduced, -1.0f32)
    } else if reduced < -FRAC_PI_2 {
        (-PI - reduced, -1.0f32)
    } else {
        (reduced, 1.0f32)
    };
    let r2 = r * r;
    let sin_val = r * (1.0 - r2 * (1.0 / 6.0 - r2 * (1.0 / 120.0 - r2 * (1.0 / 5040.0))));
    let cos_val = cos_sign * (1.0 - r2 * (0.5 - r2 * (1.0 / 24.0 - r2 * (1.0 / 720.0))));
    (sin_val, cos_val)
}

#[inline]
#[allow(clippy::approx_constant)]
fn fast_powf(base: f32, exp: f32) -> f32 {
    if base <= 0.0 {
        return 0.0;
    }
    let log2_base = {
        const P0: f32 = -1.850_383_3e-6;
        const P1: f32 = 1.428_716_1;
        const P2: f32 = 0.742_458_7;
        const Q0: f32 = 0.990_328_14;
        const Q1: f32 = 1.009_671_8;
        const Q2: f32 = 0.174_093_43;
        let x_bits = base.to_bits() as i32;
        let offset = 0x3f2a_aaab_u32 as i32;
        let exp_bits = x_bits.wrapping_sub(offset);
        let exp_shifted = exp_bits >> 23;
        let mantissa_bits = x_bits.wrapping_sub(exp_shifted << 23);
        let mantissa = f32::from_bits(mantissa_bits as u32);
        let exp_val = exp_shifted as f32;
        let m = mantissa - 1.0;
        let yp = P2 * m + P1;
        let yp = yp * m + P0;
        let yq = Q2 * m + Q1;
        let yq = yq * m + Q0;
        yp / yq + exp_val
    };
    let product = (exp * log2_base).clamp(-126.0, 126.0);
    let xi = product.floor();
    let xf = product - xi;
    let poly = 0.055_504_11 * xf + 0.240_226_5;
    let poly = poly * xf + core::f32::consts::LN_2;
    let poly = poly * xf + 1.0;
    let scale_bits = ((xi as i32 + 127) << 23) as u32;
    poly * f32::from_bits(scale_bits)
}

/// Generate varying inputs to defeat constant folding.
/// Returns N f32 values in [lo, hi].
fn make_inputs(n: usize, lo: f32, hi: f32) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1).max(1) as f32;
            lo + t * (hi - lo)
        })
        .collect()
}

fn main() {
    let n = 10_000_000usize;
    let rounds = 3;

    let bases = make_inputs(n, 0.01, 2.0);
    let angles = make_inputs(n, -3.14, 3.14);
    let ab_vals = make_inputs(n, -0.2, 0.2);

    for round in 0..rounds {
        if round > 0 {
            println!("--- round {} ---\n", round + 1);
        }

        // === powf ===
        {
            let exp = 2.4f32;
            let start = Instant::now();
            let mut sum = 0.0f32;
            for &b in &bases {
                sum += b.powf(exp);
            }
            let std_ns = start.elapsed().as_nanos() as f64 / n as f64;
            black_box(sum);

            let start = Instant::now();
            let mut sum = 0.0f32;
            for &b in &bases {
                sum += fast_powf(b, exp);
            }
            let fast_ns = start.elapsed().as_nanos() as f64 / n as f64;
            black_box(sum);

            println!("powf(x, 2.4)  std: {std_ns:5.2} ns   fast: {fast_ns:5.2} ns   speedup: {:.1}x", std_ns / fast_ns);
        }

        // === atan2 ===
        {
            let start = Instant::now();
            let mut sum = 0.0f32;
            for i in 0..n {
                let y = ab_vals[i];
                let x = ab_vals[(i + n / 3) % n];
                sum += y.atan2(x);
            }
            let std_ns = start.elapsed().as_nanos() as f64 / n as f64;
            black_box(sum);

            let start = Instant::now();
            let mut sum = 0.0f32;
            for i in 0..n {
                let y = ab_vals[i];
                let x = ab_vals[(i + n / 3) % n];
                sum += fast_atan2(y, x);
            }
            let fast_ns = start.elapsed().as_nanos() as f64 / n as f64;
            black_box(sum);

            println!("atan2(y,x)    std: {std_ns:5.2} ns   fast: {fast_ns:5.2} ns   speedup: {:.1}x", std_ns / fast_ns);
        }

        // === sin + cos ===
        {
            let start = Instant::now();
            let mut sum = 0.0f32;
            for &a in &angles {
                sum += a.sin() + a.cos();
            }
            let std_ns = start.elapsed().as_nanos() as f64 / n as f64;
            black_box(sum);

            let start = Instant::now();
            let mut sum = 0.0f32;
            for &a in &angles {
                let (s, c) = fast_sincos(a);
                sum += s + c;
            }
            let fast_ns = start.elapsed().as_nanos() as f64 / n as f64;
            black_box(sum);

            println!("sin+cos(x)    std: {std_ns:5.2} ns   fast: {fast_ns:5.2} ns   speedup: {:.1}x", std_ns / fast_ns);
        }

        // === polar roundtrip (the real HSL adjust pattern) ===
        {
            let shift = 0.3f32;
            let start = Instant::now();
            let mut sum = 0.0f32;
            for i in 0..n {
                let a = ab_vals[i];
                let b = ab_vals[(i + 1) % n];
                let chroma = (a * a + b * b).sqrt();
                let hue = b.atan2(a);
                let new_hue = hue + shift;
                sum += chroma * new_hue.cos() + chroma * new_hue.sin();
            }
            let std_ns = start.elapsed().as_nanos() as f64 / n as f64;
            black_box(sum);

            let start = Instant::now();
            let mut sum = 0.0f32;
            for i in 0..n {
                let a = ab_vals[i];
                let b = ab_vals[(i + 1) % n];
                let chroma = (a * a + b * b).sqrt();
                let hue = fast_atan2(b, a);
                let new_hue = hue + shift;
                let (s, c) = fast_sincos(new_hue);
                sum += chroma * c + chroma * s;
            }
            let fast_ns = start.elapsed().as_nanos() as f64 / n as f64;
            black_box(sum);

            println!("polar round   std: {std_ns:5.2} ns   fast: {fast_ns:5.2} ns   speedup: {:.1}x", std_ns / fast_ns);
        }

        println!();
    }
}
