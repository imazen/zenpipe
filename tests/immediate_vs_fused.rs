//! Immediate-mode pixel simulation vs fused layout computation.
//!
//! Every pixel in the source stores its (x, y) origin coordinates, making
//! any geometric error immediately detectable — wrong crop, wrong scale,
//! wrong placement all show up as mismatched coordinates.
//!
//! "Immediate mode" = apply each command to actual pixel data, one step at a
//! time, like a user would intuitively expect.
//!
//! "Fused mode" = compute a single Layout via compute_layout_sequential(),
//! then apply that Layout in one shot.
//!
//! Mismatches reveal where the fused layout abstraction can't faithfully
//! represent the sequential operation.

use zenlayout::*;

// ---- Pixel simulation ----

/// A pixel that remembers where it came from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Pixel {
    /// Source pixel at (x, y) in the original image.
    Source(u32, u32),
    /// Fill / padding pixel.
    Fill,
}

/// A pixel buffer for geometric validation.
#[derive(Clone, Debug)]
struct Grid {
    width: u32,
    height: u32,
    pixels: Vec<Pixel>,
}

impl Grid {
    /// Source image: pixel at (x,y) stores Source(x,y).
    fn source(w: u32, h: u32) -> Self {
        let pixels = (0..h)
            .flat_map(|y| (0..w).map(move |x| Pixel::Source(x, y)))
            .collect();
        Self {
            width: w,
            height: h,
            pixels,
        }
    }

    fn get(&self, x: u32, y: u32) -> Pixel {
        assert!(
            x < self.width && y < self.height,
            "({x},{y}) out of bounds {}x{}",
            self.width,
            self.height
        );
        self.pixels[(y * self.width + x) as usize]
    }

    /// Crop: extract sub-rectangle. Clamps to bounds.
    fn crop(&self, cx: u32, cy: u32, cw: u32, ch: u32) -> Self {
        let cx = cx.min(self.width);
        let cy = cy.min(self.height);
        let cw = cw.min(self.width.saturating_sub(cx));
        let ch = ch.min(self.height.saturating_sub(cy));
        let mut pixels = Vec::with_capacity((cw * ch) as usize);
        for y in cy..cy + ch {
            for x in cx..cx + cw {
                pixels.push(self.get(x, y));
            }
        }
        Self {
            width: cw,
            height: ch,
            pixels,
        }
    }

    /// Nearest-neighbor resize.
    fn resize_nn(&self, new_w: u32, new_h: u32) -> Self {
        assert!(new_w > 0 && new_h > 0);
        if new_w == self.width && new_h == self.height {
            return self.clone();
        }
        let mut pixels = Vec::with_capacity((new_w * new_h) as usize);
        for y in 0..new_h {
            let src_y = ((y as f64 + 0.5) * self.height as f64 / new_h as f64).floor() as u32;
            let src_y = src_y.min(self.height - 1);
            for x in 0..new_w {
                let src_x = ((x as f64 + 0.5) * self.width as f64 / new_w as f64).floor() as u32;
                let src_x = src_x.min(self.width - 1);
                pixels.push(self.get(src_x, src_y));
            }
        }
        Self {
            width: new_w,
            height: new_h,
            pixels,
        }
    }

    /// Place this grid at (px, py) on a canvas. Handles negative offsets (clipping).
    fn place_on_canvas(&self, cw: u32, ch: u32, px: i32, py: i32) -> Self {
        let mut pixels = vec![Pixel::Fill; (cw * ch) as usize];
        for sy in 0..self.height {
            let dy = py + sy as i32;
            if dy < 0 || dy >= ch as i32 {
                continue;
            }
            for sx in 0..self.width {
                let dx = px + sx as i32;
                if dx < 0 || dx >= cw as i32 {
                    continue;
                }
                pixels[(dy as u32 * cw + dx as u32) as usize] = self.get(sx, sy);
            }
        }
        Self {
            width: cw,
            height: ch,
            pixels,
        }
    }

    /// Add padding around the image.
    fn pad(&self, top: u32, right: u32, bottom: u32, left: u32) -> Self {
        let cw = self.width + left + right;
        let ch = self.height + top + bottom;
        self.place_on_canvas(cw, ch, left as i32, top as i32)
    }

    /// Flip horizontally.
    fn flip_h(&self) -> Self {
        let mut pixels = Vec::with_capacity(self.pixels.len());
        for y in 0..self.height {
            for x in (0..self.width).rev() {
                pixels.push(self.get(x, y));
            }
        }
        Self {
            width: self.width,
            height: self.height,
            pixels,
        }
    }

    /// Flip vertically.
    fn flip_v(&self) -> Self {
        let mut pixels = Vec::with_capacity(self.pixels.len());
        for y in (0..self.height).rev() {
            for x in 0..self.width {
                pixels.push(self.get(x, y));
            }
        }
        Self {
            width: self.width,
            height: self.height,
            pixels,
        }
    }

    /// Rotate 90° clockwise.
    fn rotate_90(&self) -> Self {
        let new_w = self.height;
        let new_h = self.width;
        let mut pixels = Vec::with_capacity(self.pixels.len());
        for y in 0..new_h {
            for x in 0..new_w {
                pixels.push(self.get(y, new_w - 1 - x));
            }
        }
        Self {
            width: new_w,
            height: new_h,
            pixels,
        }
    }

    /// Rotate 180°.
    fn rotate_180(&self) -> Self {
        let mut pixels = self.pixels.clone();
        pixels.reverse();
        Self {
            width: self.width,
            height: self.height,
            pixels,
        }
    }

    /// Rotate 270° clockwise.
    fn rotate_270(&self) -> Self {
        let new_w = self.height;
        let new_h = self.width;
        let mut pixels = Vec::with_capacity(self.pixels.len());
        for y in 0..new_h {
            for x in 0..new_w {
                pixels.push(self.get(new_h - 1 - y, x));
            }
        }
        Self {
            width: new_w,
            height: new_h,
            pixels,
        }
    }

    /// Apply an Orientation transform.
    fn apply_orientation(&self, o: Orientation) -> Self {
        match o {
            Orientation::Identity => self.clone(),
            Orientation::FlipH => self.flip_h(),
            Orientation::Rotate180 => self.rotate_180(),
            Orientation::FlipV => self.flip_v(),
            Orientation::Transpose => self.rotate_90().flip_h(),
            Orientation::Rotate90 => self.rotate_90(),
            Orientation::Transverse => self.rotate_270().flip_h(),
            Orientation::Rotate270 => self.rotate_270(),
            _ => panic!("unknown orientation variant"),
        }
    }

    /// Apply a Region viewport: crop source overlap, place on viewport canvas.
    fn apply_region(&self, reg: &Region) -> Self {
        let left = reg.left.resolve(self.width);
        let top = reg.top.resolve(self.height);
        let right = reg.right.resolve(self.width);
        let bottom = reg.bottom.resolve(self.height);

        let vw = (right - left).max(0) as u32;
        let vh = (bottom - top).max(0) as u32;
        if vw == 0 || vh == 0 {
            return Self {
                width: 0,
                height: 0,
                pixels: vec![],
            };
        }

        // Compute source overlap
        let ol = left.max(0) as u32;
        let ot = top.max(0) as u32;
        let or_ = (right.min(self.width as i32)).max(0) as u32;
        let ob = (bottom.min(self.height as i32)).max(0) as u32;

        if ol >= or_ || ot >= ob {
            // No overlap — blank canvas
            return Self {
                width: vw,
                height: vh,
                pixels: vec![Pixel::Fill; (vw * vh) as usize],
            };
        }

        // Crop the source overlap
        let overlap = self.crop(ol, ot, or_ - ol, ob - ot);
        // Place on viewport canvas
        let place_x = ol as i32 - left;
        let place_y = ot as i32 - top;
        overlap.place_on_canvas(vw, vh, place_x, place_y)
    }

    /// Apply a fused Layout to the original source grid.
    fn apply_layout(&self, layout: &Layout) -> Self {
        // 1. Crop
        let cropped = if let Some(sc) = &layout.source_crop {
            self.crop(sc.x, sc.y, sc.width, sc.height)
        } else {
            self.clone()
        };

        // 2. Resize
        let resized = cropped.resize_nn(layout.resize_to.width, layout.resize_to.height);

        // 3. Place on canvas
        let (px, py) = layout.placement;
        resized.place_on_canvas(layout.canvas.width, layout.canvas.height, px, py)
    }

    fn summary(&self) -> String {
        let mut s = format!("{}x{}\n", self.width, self.height);
        for y in 0..self.height.min(16) {
            for x in 0..self.width.min(16) {
                match self.get(x, y) {
                    Pixel::Source(sx, sy) => s.push_str(&format!("({sx:2},{sy:2}) ")),
                    Pixel::Fill => s.push_str("  ..   "),
                }
            }
            s.push('\n');
        }
        if self.width > 16 || self.height > 16 {
            s.push_str("...(truncated)\n");
        }
        s
    }
}

/// Immediate-mode: apply a sequence of commands to pixels step by step.
fn immediate_eval(source: &Grid, commands: &[Command]) -> Grid {
    let mut current = source.clone();

    for cmd in commands {
        match cmd {
            Command::AutoOrient(exif) => {
                if let Some(o) = Orientation::from_exif(*exif) {
                    current = current.apply_orientation(o);
                }
            }
            Command::Rotate(r) => {
                let o = match r {
                    Rotation::Rotate90 => Orientation::Rotate90,
                    Rotation::Rotate180 => Orientation::Rotate180,
                    Rotation::Rotate270 => Orientation::Rotate270,
                    _ => unreachable!(),
                };
                current = current.apply_orientation(o);
            }
            Command::Flip(axis) => {
                let o = match axis {
                    FlipAxis::Horizontal => Orientation::FlipH,
                    FlipAxis::Vertical => Orientation::FlipV,
                    _ => unreachable!(),
                };
                current = current.apply_orientation(o);
            }
            Command::Crop(sc) => {
                let r = sc.resolve(current.width, current.height);
                current = current.crop(r.x, r.y, r.width, r.height);
            }
            Command::Region(reg) => {
                current = current.apply_region(reg);
            }
            Command::Constrain(constraint) => {
                let layout = constraint
                    .clone()
                    .compute(current.width, current.height)
                    .unwrap();
                // In immediate mode, the constraint's source is the current buffer.
                // source_crop applies to current, then resize, then place on canvas.
                let cropped = if let Some(sc) = &layout.source_crop {
                    current.crop(sc.x, sc.y, sc.width, sc.height)
                } else {
                    current
                };
                let resized = cropped.resize_nn(layout.resize_to.width, layout.resize_to.height);
                current = resized.place_on_canvas(
                    layout.canvas.width,
                    layout.canvas.height,
                    layout.placement.0,
                    layout.placement.1,
                );
            }
            Command::Pad(p) => {
                current = current.pad(p.top, p.right, p.bottom, p.left);
            }
            _ => {}
        }
    }
    current
}

/// Fused-mode: compute a single Layout via compute_layout_sequential,
/// apply it in one pass to the original source.
fn fused_eval(source: &Grid, commands: &[Command]) -> Result<Grid, At<LayoutError>> {
    let (ideal, _request) = compute_layout_sequential(commands, source.width, source.height, None)?;

    // Apply orientation to source first (layout expects oriented source)
    let oriented = source.apply_orientation(ideal.orientation);
    let result = oriented.apply_layout(&ideal.layout);
    Ok(result)
}

/// Assert exact pixel-level match between immediate and fused modes.
fn compare(name: &str, source: &Grid, commands: &[Command]) {
    let immediate = immediate_eval(source, commands);
    let fused = fused_eval(source, commands);

    match fused {
        Ok(fused) => {
            let match_ok = immediate.width == fused.width
                && immediate.height == fused.height
                && immediate.pixels == fused.pixels;

            if !match_ok {
                eprintln!("=== MISMATCH: {name} ===");
                eprintln!("Immediate ({}x{}):", immediate.width, immediate.height);
                eprintln!("{}", immediate.summary());
                eprintln!("Fused ({}x{}):", fused.width, fused.height);
                eprintln!("{}", fused.summary());

                if immediate.width == fused.width && immediate.height == fused.height {
                    let mut mismatches = 0;
                    for i in 0..immediate.pixels.len() {
                        if immediate.pixels[i] != fused.pixels[i] {
                            mismatches += 1;
                        }
                    }
                    eprintln!("{mismatches}/{} pixels differ", immediate.pixels.len());
                }
                panic!("{name}: immediate != fused");
            }
        }
        Err(e) => {
            panic!(
                "{name}: fused eval failed with {e}, but immediate produced {}x{}",
                immediate.width, immediate.height
            );
        }
    }
}

/// Assert dimensions match (pixels may differ due to NN sampling grid).
///
/// Used for cases where the single-pass layout decomposition produces the
/// correct geometry but NN resampling hits different source pixels because
/// of operation ordering (rotate-then-resize vs resize-then-rotate, etc).
fn compare_dims_match(name: &str, source: &Grid, commands: &[Command]) {
    let immediate = immediate_eval(source, commands);
    let fused = fused_eval(source, commands).unwrap();
    assert_eq!(
        (immediate.width, immediate.height),
        (fused.width, fused.height),
        "{name}: dimensions must match"
    );
}

/// Document a known design divergence — both modes must succeed.
///
/// Used for cases where the sequential evaluator's semantics intentionally
/// differ from step-by-step immediate execution (e.g., orientation always
/// fuses across crops, last-constrain-wins drops intermediate ops).
fn compare_divergent(name: &str, source: &Grid, commands: &[Command]) {
    let _immediate = immediate_eval(source, commands);
    let fused = fused_eval(source, commands);
    assert!(fused.is_ok(), "{name}: fused mode should succeed");
}

/// Classify a sequence: exact match, dimension match, or divergent.
fn compare_expect_mismatch(name: &str, source: &Grid, commands: &[Command]) -> Option<String> {
    let immediate = immediate_eval(source, commands);
    let fused = match fused_eval(source, commands) {
        Ok(f) => f,
        Err(e) => {
            return Some(format!(
                "{name}: fused error '{e}', immediate produced {}x{}",
                immediate.width, immediate.height
            ));
        }
    };

    let match_ok = immediate.width == fused.width
        && immediate.height == fused.height
        && immediate.pixels == fused.pixels;

    if match_ok {
        None
    } else {
        let mut msg = format!(
            "{name}: size immediate={}x{} fused={}x{}",
            immediate.width, immediate.height, fused.width, fused.height
        );
        if immediate.width == fused.width && immediate.height == fused.height {
            let mismatches = immediate
                .pixels
                .iter()
                .zip(&fused.pixels)
                .filter(|(a, b)| a != b)
                .count();
            msg.push_str(&format!(
                ", {mismatches}/{} pixels differ",
                immediate.pixels.len()
            ));
        }
        Some(msg)
    }
}

// ---- Tests: cases that SHOULD match ----

#[test]
fn crop_only() {
    let src = Grid::source(8, 8);
    let commands = [Command::Crop(SourceCrop::Pixels(Rect::new(2, 2, 4, 4)))];
    compare("crop_only", &src, &commands);
}

#[test]
fn crop_crop() {
    let src = Grid::source(12, 12);
    let commands = [
        Command::Crop(SourceCrop::Pixels(Rect::new(2, 2, 8, 8))),
        Command::Crop(SourceCrop::Pixels(Rect::new(1, 1, 4, 4))),
    ];
    compare("crop→crop", &src, &commands);
}

#[test]
fn crop_constrain() {
    let src = Grid::source(100, 100);
    let commands = [
        Command::Crop(SourceCrop::Pixels(Rect::new(10, 10, 80, 80))),
        Command::Constrain(Constraint::new(ConstraintMode::Fit, 40, 40)),
    ];
    compare("crop→constrain", &src, &commands);
}

#[test]
fn constrain_pad() {
    let src = Grid::source(100, 100);
    let commands = [
        Command::Constrain(Constraint::new(ConstraintMode::Fit, 50, 50)),
        Command::Pad(Padding::uniform(5, CanvasColor::Transparent)),
    ];
    compare("constrain→pad", &src, &commands);
}

#[test]
fn orient_crop_constrain() {
    let src = Grid::source(12, 8);
    let commands = [
        Command::Rotate(Rotation::Rotate90),
        Command::Crop(SourceCrop::Pixels(Rect::new(1, 1, 6, 10))),
        Command::Constrain(Constraint::new(ConstraintMode::Fit, 3, 5)),
    ];
    compare("orient→crop→constrain", &src, &commands);
}

#[test]
fn constrain_only() {
    let src = Grid::source(100, 50);
    let commands = [Command::Constrain(Constraint::new(
        ConstraintMode::Fit,
        50,
        50,
    ))];
    compare("constrain_only", &src, &commands);
}

#[test]
fn region_pure_crop() {
    let src = Grid::source(10, 10);
    let commands = [Command::Region(Region::crop(2, 2, 8, 8))];
    compare("region_pure_crop", &src, &commands);
}

#[test]
fn region_pure_pad() {
    let src = Grid::source(8, 8);
    let commands = [Command::Region(Region::padded(2, CanvasColor::Transparent))];
    compare("region_pure_pad", &src, &commands);
}

#[test]
fn region_mixed_crop_pad() {
    // Crop right side, pad left side
    let src = Grid::source(10, 10);
    let commands = [Command::Region(Region {
        left: RegionCoord::px(-3),
        top: RegionCoord::px(0),
        right: RegionCoord::px(7),
        bottom: RegionCoord::pct(1.0),
        color: CanvasColor::Transparent,
    })];
    compare("region_mixed_crop_pad", &src, &commands);
}

// ---- Tests: cases that may NOT match (structural limitations) ----

#[test]
fn constrain_then_crop_origin() {
    // Post-constrain crop at origin — should work since placement stays non-negative
    let src = Grid::source(100, 100);
    let commands = [
        Command::Constrain(Constraint::new(ConstraintMode::Fit, 50, 50)),
        Command::Crop(SourceCrop::Pixels(Rect::new(0, 0, 25, 25))),
    ];
    compare("constrain→crop(origin)", &src, &commands);
}

#[test]
fn constrain_then_crop_center() {
    // Post-constrain crop NOT at origin — placement goes negative, u32 saturates
    let src = Grid::source(100, 100);
    let commands = [
        Command::Constrain(Constraint::new(ConstraintMode::Fit, 50, 50)),
        Command::Crop(SourceCrop::Pixels(Rect::new(10, 10, 30, 30))),
    ];
    let mismatch = compare_expect_mismatch("constrain→crop(center)", &src, &commands);
    if let Some(msg) = &mismatch {
        eprintln!("EXPECTED MISMATCH (u32 placement can't go negative): {msg}");
    }
    // Fixed: i32 placement allows negative offsets.
    assert!(
        mismatch.is_none(),
        "constrain→crop(center) should match now that placement is i32"
    );
}

#[test]
fn constrain_then_crop_center_pixel_detail() {
    // Post-constrain crop with nonzero origin: i32 placement handles negative offsets
    let src = Grid::source(10, 10);
    let commands = [
        Command::Constrain(Constraint::new(ConstraintMode::Fit, 10, 10)), // identity resize
        Command::Crop(SourceCrop::Pixels(Rect::new(3, 3, 4, 4))),
    ];
    compare("constrain(identity)→crop(3,3,4,4)", &src, &commands);
}

#[test]
fn pad_region_then_constrain() {
    // Pad then resize: constraint now targets viewport dimensions.
    // For this case (8x8 source, pad 4 → 16x16, Fit 8x8), the constraint
    // targets 16x16 with scale 0.5, producing 8x8 canvas with 4x4 content
    // at (2,2). This matches immediate mode: pad to 16x16, resize to 8x8.
    let src = Grid::source(8, 8);
    let commands = [
        Command::Region(Region::padded(4, CanvasColor::Transparent)),
        Command::Constrain(Constraint::new(ConstraintMode::Fit, 8, 8)),
    ];
    compare("pad(4)→fit(8x8) on 8x8", &src, &commands);
}

#[test]
fn pad_region_then_constrain_downscale() {
    // Source 16x16, pad 4 → 24x24 viewport, Fit(8x8) → 8x8 output.
    // Dimensions match, but NN pixel coordinates differ because the
    // single-pass layout resizes content separately from padding.
    let src = Grid::source(16, 16);
    let commands = [
        Command::Region(Region::padded(4, CanvasColor::Transparent)),
        Command::Constrain(Constraint::new(ConstraintMode::Fit, 8, 8)),
    ];

    let immediate = immediate_eval(&src, &commands);
    let fused = fused_eval(&src, &commands).unwrap();

    // Dimensions MUST match (constraint targets viewport → 8x8).
    assert_eq!(
        (immediate.width, immediate.height),
        (fused.width, fused.height),
        "pad+constrain dimensions should match"
    );
    // Pixel-level differences are expected: NN resampling a padded viewport
    // vs content-only produces different sampling grids at the boundary.
}

#[test]
fn crop_constrain_crop() {
    // Pre-constrain crop + post-constrain crop: i32 placement handles the offset
    let src = Grid::source(20, 20);
    let commands = [
        Command::Crop(SourceCrop::Pixels(Rect::new(2, 2, 16, 16))),
        Command::Constrain(Constraint::new(ConstraintMode::Fit, 16, 16)),
        Command::Crop(SourceCrop::Pixels(Rect::new(4, 4, 8, 8))),
    ];
    compare("crop→constrain→crop(center)", &src, &commands);
}

#[test]
fn constrain_then_region_viewport() {
    // Post-constrain region: redefine canvas viewport of resized output
    let src = Grid::source(20, 20);
    let commands = [
        Command::Constrain(Constraint::new(ConstraintMode::Fit, 10, 10)),
        Command::Region(Region {
            left: RegionCoord::px(2),
            top: RegionCoord::px(2),
            right: RegionCoord::px(8),
            bottom: RegionCoord::px(8),
            color: CanvasColor::Transparent,
        }),
    ];
    compare("constrain→region(2,2,8,8)", &src, &commands);
}

#[test]
fn constrain_then_pad_then_crop() {
    // Constrain → pad → crop the padded result: i32 placement handles the offset
    let src = Grid::source(20, 20);
    let commands = [
        Command::Constrain(Constraint::new(ConstraintMode::Fit, 10, 10)),
        Command::Pad(Padding::uniform(5, CanvasColor::Transparent)),
        Command::Crop(SourceCrop::Pixels(Rect::new(2, 2, 16, 16))),
    ];
    compare("constrain→pad→crop", &src, &commands);
}

// ---- Creative multi-op sequences ----

// Helper to make constraint commands less verbose
fn fit(w: u32, h: u32) -> Command {
    Command::Constrain(Constraint::new(ConstraintMode::Fit, w, h))
}

fn fit_crop(w: u32, h: u32) -> Command {
    Command::Constrain(Constraint::new(ConstraintMode::FitCrop, w, h))
}

fn fit_pad(w: u32, h: u32) -> Command {
    Command::Constrain(Constraint::new(ConstraintMode::FitPad, w, h))
}

fn within(w: u32, h: u32) -> Command {
    Command::Constrain(Constraint::new(ConstraintMode::Within, w, h))
}

fn distort(w: u32, h: u32) -> Command {
    Command::Constrain(Constraint::new(ConstraintMode::Distort, w, h))
}

fn width_only(w: u32) -> Command {
    Command::Constrain(Constraint::width_only(ConstraintMode::Fit, w))
}

fn height_only(h: u32) -> Command {
    Command::Constrain(Constraint::height_only(ConstraintMode::Fit, h))
}

fn crop(x: u32, y: u32, w: u32, h: u32) -> Command {
    Command::Crop(SourceCrop::Pixels(Rect::new(x, y, w, h)))
}

fn pct_crop(x: f32, y: f32, w: f32, h: f32) -> Command {
    Command::Crop(SourceCrop::Percent {
        x,
        y,
        width: w,
        height: h,
    })
}

fn rot90() -> Command {
    Command::Rotate(Rotation::Rotate90)
}

fn rot180() -> Command {
    Command::Rotate(Rotation::Rotate180)
}

fn rot270() -> Command {
    Command::Rotate(Rotation::Rotate270)
}

fn flip_h() -> Command {
    Command::Flip(FlipAxis::Horizontal)
}

fn flip_v() -> Command {
    Command::Flip(FlipAxis::Vertical)
}

fn pad(top: u32, right: u32, bottom: u32, left: u32) -> Command {
    Command::Pad(Padding::new(
        top,
        right,
        bottom,
        left,
        CanvasColor::Transparent,
    ))
}

fn exif(val: u8) -> Command {
    Command::AutoOrient(val)
}

fn region(l: RegionCoord, t: RegionCoord, r: RegionCoord, b: RegionCoord) -> Command {
    Command::Region(Region {
        left: l,
        top: t,
        right: r,
        bottom: b,
        color: CanvasColor::Transparent,
    })
}

// --- Orientation stress ---

#[test]
fn triple_rotation_compose() {
    // 90 + 90 + 90 = 270
    let src = Grid::source(12, 8);
    compare("rot90→rot90→rot90", &src, &[rot90(), rot90(), rot90()]);
}

#[test]
fn four_rotations_identity() {
    // 90 * 4 = 360 = identity
    let src = Grid::source(15, 10);
    compare(
        "rot90×4=identity",
        &src,
        &[rot90(), rot90(), rot90(), rot90()],
    );
}

#[test]
fn flip_flip_identity() {
    // FlipH + FlipH = identity
    let src = Grid::source(7, 11);
    compare("flipH→flipH=identity", &src, &[flip_h(), flip_h()]);
}

#[test]
fn flip_h_flip_v_is_rot180() {
    // FlipH + FlipV = Rot180
    let src = Grid::source(9, 6);
    let seq1 = immediate_eval(&src, &[flip_h(), flip_v()]);
    let seq2 = immediate_eval(&src, &[rot180()]);
    assert_eq!(seq1.pixels, seq2.pixels, "flipH+flipV should equal rot180");

    compare("flipH→flipV", &src, &[flip_h(), flip_v()]);
}

#[test]
fn all_8_exif_with_crop() {
    // Every EXIF orientation followed by the same crop
    let src = Grid::source(16, 12);
    for exif_val in 1..=8 {
        let commands = [exif(exif_val), crop(1, 1, 6, 4)];
        compare(&format!("exif({exif_val})→crop"), &src, &commands);
    }
}

#[test]
fn all_8_exif_with_constrain() {
    let src = Grid::source(20, 14);
    for exif_val in 1..=8 {
        compare(
            &format!("exif({exif_val})→fit(10,10)"),
            &src,
            &[exif(exif_val), fit(10, 10)],
        );
    }
}

#[test]
fn exif_compose_with_manual_rotation() {
    // EXIF 6 (Rotate90) + manual Rotate270 = identity
    let src = Grid::source(10, 15);
    compare("exif(6)+rot270=identity", &src, &[exif(6), rot270()]);
}

#[test]
fn exif_8_then_flip_then_crop() {
    // EXIF 8 = Rotate270, then flip horizontal, then crop
    let src = Grid::source(12, 8);
    compare(
        "exif(8)→flipH→crop",
        &src,
        &[exif(8), flip_h(), crop(1, 1, 4, 6)],
    );
}

#[test]
fn transpose_then_crop() {
    // EXIF 5 = Transpose = Rot90 + FlipH
    let src = Grid::source(10, 6);
    compare("transpose→crop", &src, &[exif(5), crop(0, 0, 4, 8)]);
}

#[test]
fn transverse_then_constrain() {
    // EXIF 7 = Transverse = Rot270 + FlipH
    let src = Grid::source(14, 9);
    compare("transverse→fit", &src, &[exif(7), fit(7, 7)]);
}

// --- Constraint modes ---

#[test]
fn fit_crop_landscape_to_portrait() {
    let src = Grid::source(20, 10);
    compare("fitcrop(8,16) on 20x10", &src, &[fit_crop(8, 16)]);
}

#[test]
fn fit_crop_portrait_to_landscape() {
    let src = Grid::source(10, 20);
    compare("fitcrop(16,8) on 10x20", &src, &[fit_crop(16, 8)]);
}

#[test]
fn fit_pad_landscape_to_square() {
    let src = Grid::source(20, 10);
    compare("fitpad(15,15) on 20x10", &src, &[fit_pad(15, 15)]);
}

#[test]
fn fit_pad_portrait_to_square() {
    let src = Grid::source(10, 20);
    compare("fitpad(15,15) on 10x20", &src, &[fit_pad(15, 15)]);
}

#[test]
fn distort_stretch() {
    let src = Grid::source(10, 10);
    compare("distort(20,5) on 10x10", &src, &[distort(20, 5)]);
}

#[test]
fn distort_then_crop() {
    let src = Grid::source(12, 8);
    compare("distort→crop", &src, &[distort(24, 4), crop(4, 0, 16, 4)]);
}

#[test]
fn within_no_upscale() {
    // Within mode: source smaller than target → no resize
    let src = Grid::source(5, 5);
    compare("within(20,20) on 5x5", &src, &[within(20, 20)]);
}

#[test]
fn within_downscale() {
    let src = Grid::source(40, 20);
    compare("within(10,10) on 40x20", &src, &[within(10, 10)]);
}

#[test]
fn width_only_constraint() {
    let src = Grid::source(20, 10);
    compare("width_only(8) on 20x10", &src, &[width_only(8)]);
}

#[test]
fn height_only_constraint() {
    let src = Grid::source(10, 20);
    compare("height_only(8) on 10x20", &src, &[height_only(8)]);
}

// --- Multi-constrain (last wins in sequential) ---
//
// In sequential mode, "last constrain wins" means prior constrains and their
// post-ops are discarded. The final constrain operates on the original source.
// This means intermediate resizes don't cascade, which differs from immediate
// mode where each constrain resizes the current buffer.
//
// Consequences:
// - NN pixel differences when dimensions match (different sampling grids)
// - Dimension differences when cascade rounding changes aspect ratio

#[test]
fn two_constrains_last_wins() {
    // 20→10→6 (immediate) vs 20→6 (fused): same 6×6 dims, different NN grid
    let src = Grid::source(20, 20);
    compare_dims_match("fit(10)→fit(6) last wins", &src, &[fit(10, 10), fit(6, 6)]);
}

#[test]
fn three_constrains_last_wins() {
    // 30×20→15×10→10×7→5×4 (immediate) vs 30×20→5×3 (fused)
    // Dimensions differ: cascade rounding produces 5×4, direct produces 5×3
    let src = Grid::source(30, 20);
    compare_divergent(
        "fit(15)→fit(10)→fit(5) last wins",
        &src,
        &[fit(15, 15), fit(10, 10), fit(5, 5)],
    );
}

#[test]
fn constrain_mode_switch_last_wins() {
    // FitCrop crops to aspect then resizes. In immediate, fitcrop operates first.
    // In fused, only the last (distort) operates on original source.
    let src = Grid::source(20, 10);
    compare_dims_match(
        "fitcrop(10,10)→distort(8,4)",
        &src,
        &[fit_crop(10, 10), distort(8, 4)],
    );
}

// --- Crop gymnastics ---

#[test]
fn triple_crop() {
    let src = Grid::source(20, 20);
    compare(
        "crop³",
        &src,
        &[crop(2, 2, 16, 16), crop(2, 2, 12, 12), crop(2, 2, 8, 8)],
    );
}

#[test]
fn crop_to_1x1_then_constrain() {
    let src = Grid::source(10, 10);
    compare("crop(1x1)→fit(1,1)", &src, &[crop(5, 5, 1, 1), fit(1, 1)]);
}

#[test]
fn percent_crop_then_constrain() {
    // Crop center 50%
    let src = Grid::source(20, 20);
    compare(
        "pct_crop(25%,25%,50%,50%)→fit(5,5)",
        &src,
        &[pct_crop(0.25, 0.25, 0.5, 0.5), fit(5, 5)],
    );
}

#[test]
fn percent_crop_90_percent() {
    let src = Grid::source(20, 20);
    compare(
        "pct_crop(5%,5%,90%,90%)",
        &src,
        &[pct_crop(0.05, 0.05, 0.9, 0.9)],
    );
}

#[test]
fn crop_then_pad_then_crop() {
    // In fused mode: pad between two crops goes to post_ops, the two crops
    // compose in source space (ignoring the pad). Second crop extends beyond
    // first crop's viewport, creating implicit padding. Then the explicit pad
    // adds more. Result: different dimensions and content.
    let src = Grid::source(16, 16);
    compare_divergent(
        "crop→pad→crop",
        &src,
        &[crop(4, 4, 8, 8), pad(3, 3, 3, 3), crop(1, 1, 12, 12)],
    );
}

// --- Flip sandwich: flip, do something asymmetric, flip back ---
//
// In sequential mode, ALL orientation commands fuse algebraically regardless
// of position. FlipH + FlipH = Identity. The crop between them operates in
// the un-flipped space (since the net orientation is identity).
//
// In immediate mode, the first flip affects which pixels the crop selects,
// and the second flip mirrors the cropped result back.
//
// This is a fundamental design choice: orientation fusion enables single-pass
// layout computation but can't represent "flip, crop, flip back."

#[test]
fn flip_crop_flip_vs_mirror_crop() {
    // Immediate: flip→crop left→flip = right side of source
    // Fused: FlipH∘FlipH=Identity, crop left = left side of source
    let src = Grid::source(12, 8);
    compare_divergent(
        "flipH→crop(0,0,6,8)→flipH",
        &src,
        &[flip_h(), crop(0, 0, 6, 8), flip_h()],
    );
}

#[test]
fn flip_v_crop_flip_v() {
    // Immediate: flip→crop top→flip = bottom of source
    // Fused: FlipV∘FlipV=Identity, crop top = top of source
    let src = Grid::source(8, 12);
    compare_divergent(
        "flipV→crop(0,0,8,6)→flipV",
        &src,
        &[flip_v(), crop(0, 0, 8, 6), flip_v()],
    );
}

// --- Extreme aspect ratios ---

#[test]
fn stick_thin_fit() {
    let src = Grid::source(1, 100);
    compare("fit(10,10) on 1x100 stick", &src, &[fit(10, 10)]);
}

#[test]
fn stick_wide_fit() {
    let src = Grid::source(100, 1);
    compare("fit(10,10) on 100x1 ribbon", &src, &[fit(10, 10)]);
}

#[test]
fn stick_thin_fit_crop() {
    let src = Grid::source(2, 50);
    compare("fitcrop(10,10) on 2x50", &src, &[fit_crop(10, 10)]);
}

#[test]
fn stick_wide_distort() {
    let src = Grid::source(50, 2);
    compare("distort(5,20) on 50x2", &src, &[distort(5, 20)]);
}

// --- Asymmetric padding ---

#[test]
fn asymmetric_pad_only() {
    let src = Grid::source(8, 6);
    compare("pad(1,2,3,4)", &src, &[pad(1, 2, 3, 4)]);
}

#[test]
fn asymmetric_pad_then_crop() {
    let src = Grid::source(8, 6);
    compare(
        "pad(1,2,3,4)→crop(2,0,10,8)",
        &src,
        &[pad(1, 2, 3, 4), crop(2, 0, 10, 8)],
    );
}

#[test]
fn constrain_then_asymmetric_pad() {
    let src = Grid::source(20, 10);
    compare(
        "fit(10,5)→pad(0,0,5,0)",
        &src,
        &[fit(10, 5), pad(0, 0, 5, 0)],
    );
}

// --- Region creativity ---

#[test]
fn region_crop_left_pad_right() {
    // Crop 3px from left, pad 3px on right
    let src = Grid::source(10, 10);
    compare(
        "region: crop_left+pad_right",
        &src,
        &[region(
            RegionCoord::px(3),
            RegionCoord::px(0),
            RegionCoord::pct_px(1.0, 3),
            RegionCoord::pct(1.0),
        )],
    );
}

#[test]
fn region_pct_px_mixed() {
    // Left at 10%+5px, top at 0, right at 90%-5px, bottom at 100%
    let src = Grid::source(20, 20);
    compare(
        "region(10%+5,0,90%-5,100%)",
        &src,
        &[region(
            RegionCoord::pct_px(0.1, 5),
            RegionCoord::px(0),
            RegionCoord::pct_px(0.9, -5),
            RegionCoord::pct(1.0),
        )],
    );
}

#[test]
fn region_pad_top_only() {
    // Only pad the top by 10px
    let src = Grid::source(8, 8);
    compare(
        "region: pad top 10",
        &src,
        &[region(
            RegionCoord::px(0),
            RegionCoord::px(-10),
            RegionCoord::pct(1.0),
            RegionCoord::pct(1.0),
        )],
    );
}

#[test]
fn region_crop_then_crop_compose() {
    // Two region crops compose: first takes center, second refines
    let src = Grid::source(20, 20);
    compare(
        "region_crop→crop compose",
        &src,
        &[
            Command::Region(Region::crop(2, 2, 18, 18)),
            crop(1, 1, 14, 14),
        ],
    );
}

#[test]
fn region_mixed_then_constrain() {
    // Region that pads top and crops bottom, then constrain.
    // Padded viewport + constraint = NN boundary artifact at content/padding edge.
    // Padding ratio 3/10 → at 5px output, rounding differs (1 vs 2 fill rows).
    let src = Grid::source(10, 10);
    compare_dims_match(
        "region(0,-3,10,7)→fit(5,5)",
        &src,
        &[
            region(
                RegionCoord::px(0),
                RegionCoord::px(-3),
                RegionCoord::pct(1.0),
                RegionCoord::px(7),
            ),
            fit(5, 5),
        ],
    );
}

// --- Long pipelines (5+ ops) ---

#[test]
fn rotate_crop_constrain_pad_crop() {
    let src = Grid::source(20, 12);
    compare(
        "rot90→crop→fit→pad→crop",
        &src,
        &[
            rot90(),
            crop(1, 1, 10, 18),
            fit(5, 9),
            pad(2, 2, 2, 2),
            crop(1, 1, 7, 11),
        ],
    );
}

#[test]
fn exif_crop_constrain_flip_pad() {
    // Post-constrain flip: NN grid artifact from orientation fusing
    let src = Grid::source(16, 12);
    compare_dims_match(
        "exif(3)→crop→fit→flipH→pad",
        &src,
        &[
            exif(3),
            crop(2, 2, 12, 8),
            fit(6, 4),
            flip_h(),
            pad(1, 1, 1, 1),
        ],
    );
}

#[test]
fn crop_crop_constrain_pad_pad() {
    let src = Grid::source(30, 30);
    compare(
        "crop→crop→fit→pad→pad",
        &src,
        &[
            crop(5, 5, 20, 20),
            crop(2, 2, 16, 16),
            fit(8, 8),
            pad(1, 1, 1, 1),
            pad(2, 2, 2, 2),
        ],
    );
}

#[test]
fn orient_region_constrain_crop() {
    let src = Grid::source(16, 12);
    compare(
        "rot270→region(1,1,11,7)→fit(5,3)→crop(0,0,4,3)",
        &src,
        &[
            rot270(),
            Command::Region(Region::crop(1, 1, 11, 7)),
            fit(5, 3),
            crop(0, 0, 4, 3),
        ],
    );
}

// --- Post-constrain orientation (Category D) ---
//
// Post-constrain orientation commands are fused into the pre-orientation
// (applied to source before resize). In immediate mode, the orientation
// is applied AFTER the resize. NN(orient(source)) ≠ orient(NN(source))
// because the center-offset sampling formula picks different source pixels.
//
// Dimensions always match. With real resamplers the differences vanish.

#[test]
fn constrain_then_flip_h() {
    let src = Grid::source(12, 8);
    compare_dims_match("fit(6,4)→flipH", &src, &[fit(6, 4), flip_h()]);
}

#[test]
fn constrain_then_flip_v() {
    let src = Grid::source(12, 8);
    compare_dims_match("fit(6,4)→flipV", &src, &[fit(6, 4), flip_v()]);
}

#[test]
fn constrain_then_rot180() {
    let src = Grid::source(12, 8);
    compare_dims_match("fit(6,4)→rot180", &src, &[fit(6, 4), rot180()]);
}

#[test]
fn constrain_then_flip_then_crop() {
    // NN grid offset from post-constrain flip + post-constrain crop
    let src = Grid::source(16, 10);
    compare_dims_match(
        "fit(8,5)→flipH→crop(0,0,4,5)",
        &src,
        &[fit(8, 5), flip_h(), crop(0, 0, 4, 5)],
    );
}

#[test]
fn constrain_then_flip_then_pad() {
    let src = Grid::source(12, 8);
    compare_dims_match(
        "fit(6,4)→flipV→pad(2,2,2,2)",
        &src,
        &[fit(6, 4), flip_v(), pad(2, 2, 2, 2)],
    );
}

#[test]
fn constrain_then_double_flip() {
    // FlipH + FlipH = identity post-orientation, should not affect output
    let src = Grid::source(12, 8);
    compare(
        "fit(6,4)→flipH→flipH=identity",
        &src,
        &[fit(6, 4), flip_h(), flip_h()],
    );
}

// --- Two constrains with post-ops between ---
//
// "Last constrain wins" means ALL prior constrains and their post-ops are
// discarded. The final constrain operates directly on the original source.
// This means intermediate resizes, crops, pads, and flips between constrains
// don't affect the fused output — a fundamental design choice that enables
// single-pass layout computation.

#[test]
fn constrain_pad_constrain() {
    // Immediate: resize→pad→resize (pad preserved between resizes)
    // Fused: last constrain on original source, pad lost (post_ops cleared)
    let src = Grid::source(20, 20);
    compare_divergent(
        "fit(10,10)→pad(5,5,5,5)→fit(8,8)",
        &src,
        &[fit(10, 10), pad(5, 5, 5, 5), fit(8, 8)],
    );
}

#[test]
fn constrain_crop_constrain() {
    // Immediate: resize→crop→resize (crop refines resized output)
    // Fused: last constrain on original source, intermediate crop lost
    let src = Grid::source(20, 20);
    compare_dims_match(
        "fit(10,10)→crop(2,2,6,6)→fit(3,3)",
        &src,
        &[fit(10, 10), crop(2, 2, 6, 6), fit(3, 3)],
    );
}

#[test]
fn constrain_flip_constrain() {
    // Flip between constrains: post_orientation absorbed into pre-orientation
    // when second constrain appears. NN grid differs from cascade.
    let src = Grid::source(12, 8);
    compare_dims_match(
        "fit(6,4)→flipH→fit(3,2)",
        &src,
        &[fit(6, 4), flip_h(), fit(3, 2)],
    );
}

// --- Identity / no-op sequences ---

#[test]
fn identity_exif_noop() {
    // EXIF 1 = Identity
    let src = Grid::source(10, 10);
    compare("exif(1) is noop", &src, &[exif(1)]);
}

#[test]
fn invalid_exif_ignored() {
    // EXIF 0 and 9 should be ignored
    let src = Grid::source(10, 10);
    compare("exif(0) ignored", &src, &[exif(0)]);
    compare("exif(9) ignored", &src, &[exif(9)]);
}

#[test]
fn full_image_crop_is_identity() {
    let src = Grid::source(10, 8);
    compare("crop(0,0,10,8)=identity", &src, &[crop(0, 0, 10, 8)]);
}

#[test]
fn zero_pad_is_identity() {
    let src = Grid::source(10, 8);
    compare("pad(0,0,0,0)=identity", &src, &[pad(0, 0, 0, 0)]);
}

#[test]
fn fit_to_same_size_is_identity() {
    let src = Grid::source(10, 8);
    compare("fit(10,8) on 10x8", &src, &[fit(10, 8)]);
}

// --- Non-square sources ---

#[test]
fn tall_portrait_crop_constrain() {
    let src = Grid::source(6, 18);
    compare("crop→fit on 6x18", &src, &[crop(1, 3, 4, 12), fit(4, 6)]);
}

#[test]
fn wide_landscape_rotate_fit() {
    let src = Grid::source(24, 6);
    compare("rot90→fit(6,12) on 24x6", &src, &[rot90(), fit(6, 12)]);
}

#[test]
fn odd_dims_fit_crop() {
    // Prime dimensions stress rounding
    let src = Grid::source(17, 13);
    compare("fitcrop(7,5) on 17x13", &src, &[fit_crop(7, 5)]);
}

#[test]
fn odd_dims_fit_pad() {
    let src = Grid::source(13, 17);
    compare("fitpad(7,5) on 13x17", &src, &[fit_pad(7, 5)]);
}

// --- Stress: randomized-style exhaustive ---

#[test]
fn all_rotations_with_crop_on_nonsquare() {
    let src = Grid::source(15, 10);
    for r in [rot90(), rot180(), rot270()] {
        for &(cx, cy, cw, ch) in &[(0, 0, 5, 5), (2, 1, 6, 4), (0, 0, 10, 15)] {
            // After rotation, dims may swap — use a crop that fits any orientation
            let name = format!("rot→crop({cx},{cy},{cw},{ch}) on 15x10");
            // This may fail if crop exceeds rotated dims; compare_expect_mismatch handles gracefully
            let immediate = immediate_eval(&src, &[r.clone(), crop(cx, cy, cw, ch)]);
            let fused = fused_eval(&src, &[r.clone(), crop(cx, cy, cw, ch)]);
            if let Ok(f) = fused
                && immediate.width == f.width
                && immediate.height == f.height
            {
                assert_eq!(immediate.pixels, f.pixels, "mismatch: {name}");
            }
        }
    }
}

#[test]
fn all_constraint_modes_on_landscape() {
    let src = Grid::source(20, 10);
    for (name, cmd) in [
        ("fit", fit(8, 8)),
        ("fitcrop", fit_crop(8, 8)),
        ("fitpad", fit_pad(8, 8)),
        ("within", within(8, 8)),
        ("distort", distort(8, 8)),
        ("width_only", width_only(8)),
        ("height_only", height_only(8)),
    ] {
        compare(&format!("{name}(8,8) on 20x10"), &src, &[cmd]);
    }
}

#[test]
fn all_constraint_modes_on_portrait() {
    let src = Grid::source(10, 20);
    for (name, cmd) in [
        ("fit", fit(8, 8)),
        ("fitcrop", fit_crop(8, 8)),
        ("fitpad", fit_pad(8, 8)),
        ("within", within(8, 8)),
        ("distort", distort(8, 8)),
        ("width_only", width_only(8)),
        ("height_only", height_only(8)),
    ] {
        compare(&format!("{name}(8,8) on 10x20"), &src, &[cmd]);
    }
}

#[test]
fn crop_then_all_constraint_modes() {
    let src = Grid::source(20, 20);
    let c = crop(2, 2, 16, 12);
    for (name, mode) in [
        ("fit", fit(6, 6)),
        ("fitcrop", fit_crop(6, 6)),
        ("fitpad", fit_pad(6, 6)),
        ("distort", distort(6, 6)),
    ] {
        compare(&format!("crop→{name} on 20x20"), &src, &[c.clone(), mode]);
    }
}

// --- Real-world-ish scenarios ---

#[test]
fn thumbnail_pipeline() {
    // Typical thumbnail: auto-orient, fit to 150x150
    let src = Grid::source(40, 30);
    compare(
        "thumbnail: exif(6)→fit(15,15)",
        &src,
        &[exif(6), fit(15, 15)],
    );
}

#[test]
fn avatar_pipeline() {
    // Avatar: crop to square center, resize to 64x64
    let src = Grid::source(20, 14);
    compare("avatar: fitcrop(8,8)", &src, &[fit_crop(8, 8)]);
}

#[test]
fn banner_pipeline() {
    // Banner: crop top portion, resize wide
    let src = Grid::source(30, 20);
    compare(
        "banner: crop(0,0,30,10)→fit(15,5)",
        &src,
        &[crop(0, 0, 30, 10), fit(15, 5)],
    );
}

#[test]
fn letterbox_pipeline() {
    // Letterbox: fit into box, pad to exact size
    let src = Grid::source(20, 10);
    compare("letterbox: fitpad(10,10)", &src, &[fit_pad(10, 10)]);
}

#[test]
fn photo_edit_pipeline() {
    // Photo editing: orient, crop, resize, add border
    let src = Grid::source(24, 16);
    compare(
        "photo edit: exif(8)→crop→fit→pad border",
        &src,
        &[exif(8), crop(2, 2, 12, 20), fit(6, 10), pad(1, 1, 1, 1)],
    );
}

#[test]
fn watermark_canvas_pipeline() {
    // Create canvas with margin for watermark at bottom
    let src = Grid::source(20, 14);
    compare(
        "watermark canvas: fit→pad(0,0,4,0)",
        &src,
        &[fit(10, 7), pad(0, 0, 4, 0)],
    );
}

// --- Edge cases ---

#[test]
fn crop_entire_except_1px_border() {
    let src = Grid::source(10, 10);
    compare("crop(1,1,8,8)", &src, &[crop(1, 1, 8, 8)]);
}

#[test]
fn crop_single_row() {
    let src = Grid::source(10, 10);
    compare("crop single row", &src, &[crop(0, 5, 10, 1)]);
}

#[test]
fn crop_single_column() {
    let src = Grid::source(10, 10);
    compare("crop single column", &src, &[crop(5, 0, 1, 10)]);
}

#[test]
fn fit_1x1() {
    let src = Grid::source(10, 10);
    compare("fit(1,1) on 10x10", &src, &[fit(1, 1)]);
}

#[test]
fn fit_crop_1x1() {
    let src = Grid::source(10, 10);
    compare("fitcrop(1,1) on 10x10", &src, &[fit_crop(1, 1)]);
}

#[test]
fn pad_then_pad() {
    // Two pads should stack
    let src = Grid::source(6, 6);
    compare(
        "pad(1,1,1,1)→pad(2,2,2,2)",
        &src,
        &[pad(1, 1, 1, 1), pad(2, 2, 2, 2)],
    );
}

#[test]
fn region_then_region_compose() {
    // Two regions compose: first pads, second crops into the padded result
    let src = Grid::source(10, 10);
    compare(
        "region_pad→region_crop compose",
        &src,
        &[
            Command::Region(Region::padded(3, CanvasColor::Transparent)),
            Command::Region(Region::crop(1, 1, 14, 14)),
        ],
    );
}

// --- Comprehensive audit: document ALL match/mismatch classifications ---

#[test]
fn audit_all_two_op_sequences() {
    let src12 = Grid::source(12, 12);
    let src12x8 = Grid::source(12, 8);

    let crop_a = Command::Crop(SourceCrop::Pixels(Rect::new(2, 2, 8, 8)));
    let crop_b = Command::Crop(SourceCrop::Pixels(Rect::new(1, 1, 4, 4)));
    let constrain = fit(6, 6);
    let constrain_rect = fit(6, 4);
    let pad_2 = pad(2, 2, 2, 2);
    let region_pad = Command::Region(Region::padded(2, CanvasColor::Transparent));
    let region_crop = Command::Region(Region::crop(3, 3, 9, 9));
    let rotate = rot90();

    // Three classification levels:
    //   MATCH: exact pixel-for-pixel equality
    //   NN:    dimensions match, pixels differ (NN sampling grid artifact)
    //   DIVERGE: dimensions and/or content differ by design

    // ── Two-op sequences on 12×12 square source ──
    let cases_sq: Vec<(&str, &str, Vec<Command>, &Grid)> = vec![
        (
            "MATCH",
            "crop→crop",
            vec![crop_a.clone(), crop_b.clone()],
            &src12,
        ),
        (
            "MATCH",
            "crop→constrain",
            vec![crop_a.clone(), constrain.clone()],
            &src12,
        ),
        (
            "MATCH",
            "crop→pad",
            vec![crop_a.clone(), pad_2.clone()],
            &src12,
        ),
        (
            "MATCH",
            "crop→rotate",
            vec![crop_a.clone(), rotate.clone()],
            &src12,
        ),
        (
            "MATCH",
            "constrain→crop(origin)",
            vec![constrain.clone(), crop(0, 0, 3, 3)],
            &src12,
        ),
        (
            "MATCH",
            "constrain→crop(center)",
            vec![constrain.clone(), crop_b.clone()],
            &src12,
        ),
        (
            "MATCH",
            "constrain→pad",
            vec![constrain.clone(), pad_2.clone()],
            &src12,
        ),
        (
            "NN",
            "constrain→rotate",
            vec![constrain.clone(), rotate.clone()],
            &src12,
        ),
        (
            "MATCH",
            "pad→crop",
            vec![pad_2.clone(), crop_a.clone()],
            &src12,
        ),
        (
            "NN",
            "pad→constrain",
            vec![pad_2.clone(), constrain.clone()],
            &src12,
        ),
        (
            "MATCH",
            "rotate→crop",
            vec![rotate.clone(), crop_a.clone()],
            &src12,
        ),
        (
            "MATCH",
            "rotate→constrain",
            vec![rotate.clone(), constrain.clone()],
            &src12,
        ),
        (
            "MATCH",
            "rotate→pad",
            vec![rotate.clone(), pad_2.clone()],
            &src12,
        ),
        (
            "NN",
            "region_pad→constrain",
            vec![region_pad.clone(), constrain.clone()],
            &src12,
        ),
        (
            "MATCH",
            "region_crop→constrain",
            vec![region_crop.clone(), constrain.clone()],
            &src12,
        ),
        (
            "MATCH",
            "constrain→region_crop",
            vec![constrain.clone(), Command::Region(Region::crop(1, 1, 5, 5))],
            &src12,
        ),
    ];

    // ── Post-constrain orientation on 12×8 non-square ──
    let cases_orient: Vec<(&str, &str, Vec<Command>, &Grid)> = vec![
        (
            "NN",
            "fit→flipH",
            vec![constrain_rect.clone(), flip_h()],
            &src12x8,
        ),
        (
            "NN",
            "fit→flipV",
            vec![constrain_rect.clone(), flip_v()],
            &src12x8,
        ),
        (
            "NN",
            "fit→rot180",
            vec![constrain_rect.clone(), rot180()],
            &src12x8,
        ),
        (
            "NN",
            "fit→rot90 (non-sq)",
            vec![constrain_rect.clone(), rot90()],
            &src12x8,
        ),
        (
            "NN",
            "fit→rot270 (non-sq)",
            vec![constrain_rect.clone(), rot270()],
            &src12x8,
        ),
        (
            "NN",
            "fit→transpose (non-sq)",
            vec![constrain_rect.clone(), exif(5)],
            &src12x8,
        ),
        (
            "NN",
            "fit→transverse (non-sq)",
            vec![constrain_rect.clone(), exif(7)],
            &src12x8,
        ),
        (
            "MATCH",
            "fit→flipH→flipH (identity)",
            vec![constrain_rect.clone(), flip_h(), flip_h()],
            &src12x8,
        ),
    ];

    // Extra source sizes needed by multi-category tests
    let src20 = Grid::source(20, 20);
    let src30x20 = Grid::source(30, 20);
    let src20x10 = Grid::source(20, 10);
    let src8x12 = Grid::source(8, 12);
    let src16 = Grid::source(16, 16);
    let src10 = Grid::source(10, 10);

    // ── Multi-constrain last-wins ──
    let cases_multi: Vec<(&str, &str, Vec<Command>, &Grid)> = vec![
        (
            "NN",
            "fit→fit (cascade)",
            vec![fit(10, 10), fit(6, 6)],
            &src20,
        ),
        (
            "DIVERGE",
            "fit→fit→fit (rounding)",
            vec![fit(15, 15), fit(10, 10), fit(5, 5)],
            &src30x20,
        ),
        (
            "NN",
            "fitcrop→distort",
            vec![fit_crop(10, 10), distort(8, 4)],
            &src20x10,
        ),
        (
            "DIVERGE",
            "fit→pad→fit (pad lost)",
            vec![fit(10, 10), pad(5, 5, 5, 5), fit(8, 8)],
            &src20,
        ),
        (
            "NN",
            "fit→crop→fit",
            vec![fit(10, 10), crop(2, 2, 6, 6), fit(3, 3)],
            &src20,
        ),
        (
            "NN",
            "fit→flip→fit",
            vec![fit(6, 4), flip_h(), fit(3, 2)],
            &src12x8,
        ),
    ];

    // ── Orientation-fuses-across-crops ──
    let cases_fuse: Vec<(&str, &str, Vec<Command>, &Grid)> = vec![
        (
            "DIVERGE",
            "flipH→crop→flipH",
            vec![flip_h(), crop(0, 0, 6, 8), flip_h()],
            &src12x8,
        ),
        (
            "DIVERGE",
            "flipV→crop→flipV",
            vec![flip_v(), crop(0, 0, 8, 6), flip_v()],
            &src8x12,
        ),
        (
            "DIVERGE",
            "crop→pad→crop",
            vec![crop(4, 4, 8, 8), pad(3, 3, 3, 3), crop(1, 1, 12, 12)],
            &src16,
        ),
    ];

    // ── Padded viewports + constrain ──
    let cases_pad: Vec<(&str, &str, Vec<Command>, &Grid)> = vec![(
        "NN",
        "region_mixed→fit",
        vec![
            region(
                RegionCoord::px(0),
                RegionCoord::px(-3),
                RegionCoord::pct(1.0),
                RegionCoord::px(7),
            ),
            fit(5, 5),
        ],
        &src10,
    )];

    type Case<'a> = (&'a str, &'a str, Vec<Command>, &'a Grid);
    // Run all categories
    let all_groups: Vec<(&str, Vec<Case<'_>>)> = vec![
        ("Two-op (12×12)", cases_sq),
        ("Post-constrain orient (12×8)", cases_orient),
        ("Multi-constrain", cases_multi),
        ("Orientation-fuses-across", cases_fuse),
        ("Padded viewport", cases_pad),
    ];

    let mut total_match = 0;
    let mut total_nn = 0;
    let mut total_diverge = 0;
    let mut failures = vec![];

    for (group, cases) in &all_groups {
        eprintln!("\n── {group} ──");
        for (expected, name, cmds, src) in cases {
            let mismatch = compare_expect_mismatch(name, src, cmds);
            match (*expected, &mismatch) {
                ("MATCH", None) => {
                    total_match += 1;
                    eprintln!("  ✓ {name}");
                }
                ("MATCH", Some(msg)) => {
                    failures.push(format!("Expected MATCH but got mismatch: {msg}"));
                    eprintln!("  ✗ {name} (expected match!) {msg}");
                }
                ("NN", None) => {
                    // Better than expected — NN artifact didn't manifest
                    total_match += 1;
                    eprintln!("  ✓ {name} (NN expected but matched!)");
                }
                ("NN", Some(msg)) => {
                    // Verify dimensions match
                    let imm = immediate_eval(src, cmds);
                    let fus = fused_eval(src, cmds).unwrap();
                    if imm.width != fus.width || imm.height != fus.height {
                        failures.push(format!("NN expected dims match but got: {msg}"));
                        eprintln!("  ✗ {name} (dims differ!) {msg}");
                    } else {
                        total_nn += 1;
                        eprintln!("  ~ {name} (NN: {msg})");
                    }
                }
                ("DIVERGE", _) => {
                    total_diverge += 1;
                    if let Some(msg) = &mismatch {
                        eprintln!("  ≠ {name} (by design: {msg})");
                    } else {
                        eprintln!("  ✓ {name} (divergent but happened to match)");
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    eprintln!("\n=== AUDIT SUMMARY ===");
    eprintln!("  Exact match: {total_match}");
    eprintln!("  NN artifact: {total_nn} (dimensions correct)");
    eprintln!("  By design:   {total_diverge}");

    assert!(
        failures.is_empty(),
        "Audit failures:\n{}",
        failures.join("\n")
    );
}
