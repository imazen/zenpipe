//! SVG visualization of layout pipeline steps.
//!
//! Generates a vertical sequence of annotated panels showing each transformation
//! in the layout pipeline: source → crop → orient → resize → canvas → edge extend.
//!
//! # Example
//!
//! ```
//! use zenlayout::{Pipeline, DecoderOffer, svg::render_layout_svg};
//!
//! let (ideal, req) = Pipeline::new(4000, 3000)
//!     .auto_orient(6)
//!     .crop_pixels(200, 200, 3000, 2000)
//!     .fit_pad(800, 800)
//!     .plan()
//!     .unwrap();
//!
//! let offer = DecoderOffer::full_decode(4000, 3000);
//! let plan = ideal.finalize(&req, &offer);
//!
//! let svg = render_layout_svg(&ideal, &plan);
//! // svg is a complete SVG document string
//! ```

#[cfg(not(feature = "std"))]
use alloc::format;
#[cfg(not(feature = "std"))]
use alloc::string::String;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::constraint::Size;
use crate::plan::{IdealLayout, LayoutPlan};

/// Maximum pixel width for any panel in the SVG output.
const MAX_PANEL_W: f64 = 300.0;
/// Maximum pixel height for any panel in the SVG output.
const MAX_PANEL_H: f64 = 200.0;
/// Vertical gap between panels.
const PANEL_GAP: f64 = 50.0;
/// Horizontal margin.
const MARGIN_X: f64 = 50.0;
/// Top margin for first panel.
const MARGIN_TOP: f64 = 30.0;
/// Height of label text area above each panel.
const LABEL_H: f64 = 22.0;

/// What the outer (background) area of a panel represents.
#[derive(Copy, Clone, PartialEq)]
enum OuterRole {
    /// Content fills the entire panel (Source, Resize, Orient).
    ContentFill,
    /// Outer area is discarded image (Crop, Trim — faded blue).
    ImageDiscard,
    /// Outer area is added padding (Canvas — white).
    Padding,
}

/// A single step in the pipeline visualization.
struct Step {
    label: String,
    /// The overall bounding box.
    outer: Size,
    /// What the outer area represents visually.
    outer_role: OuterRole,
    /// The inner content rect within the outer box.
    /// None means the content fills the entire outer box.
    inner: Option<InnerRect>,
    /// Content dimensions to show centered inside the blue box.
    content_dims: Size,
    /// Optional annotation text below the panel.
    annotation: String,
    /// If Some, show edge-extension areas for this content size.
    show_extension: Option<Size>,
}

/// A positioned rectangle within a step's outer box.
struct InnerRect {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

/// Render a complete SVG document showing the layout pipeline step by step.
///
/// Takes the [`IdealLayout`] (phase 1 result) and [`LayoutPlan`] (phase 2 result)
/// and produces a vertical sequence of annotated panels.
///
/// Returns a complete SVG document as a string.
pub fn render_layout_svg(ideal: &IdealLayout, plan: &LayoutPlan) -> String {
    let steps = build_steps(ideal, plan);
    render_steps(&steps)
}

/// Build the sequence of pipeline steps from layout results.
///
/// Pipeline order: Source (pre-orient) → Orient → Crop → Trim → Resize → Canvas → Extend.
/// Source shows raw dimensions; Orient shows the post-orientation result;
/// Crop/Trim are in post-orient space (which is how the user specifies them).
fn build_steps(ideal: &IdealLayout, plan: &LayoutPlan) -> Vec<Step> {
    let mut steps = Vec::new();
    let layout = &ideal.layout;
    let has_orient = !ideal.orientation.is_identity();

    // Source dimensions: pre-orient (raw image) when orientation is present,
    // otherwise just layout.source.
    let source_dims = if has_orient {
        ideal
            .orientation
            .inverse()
            .transform_dimensions(layout.source.width, layout.source.height)
    } else {
        layout.source
    };

    // Step 1: Source (pre-orient raw dimensions)
    steps.push(Step {
        label: String::from("Source"),
        outer: source_dims,
        outer_role: OuterRole::ContentFill,
        inner: None,
        content_dims: source_dims,
        annotation: if has_orient {
            format!("{:?}", ideal.orientation)
        } else {
            String::new()
        },
        show_extension: None,
    });

    // Step 2: Orient (post-orient dimensions — shows the dimension swap)
    if has_orient {
        steps.push(Step {
            label: String::from("Orient"),
            outer: layout.source,
            outer_role: OuterRole::ContentFill,
            inner: None,
            content_dims: layout.source,
            annotation: format!("{:?}", ideal.orientation),
            show_extension: None,
        });
    }

    // Step 3: Crop (in post-orient space — how the user specified it)
    if let Some(crop) = &layout.source_crop {
        let crop_size = Size::new(crop.width, crop.height);
        steps.push(Step {
            label: String::from("Crop"),
            outer: layout.source,
            outer_role: OuterRole::ImageDiscard,
            inner: Some(InnerRect {
                x: crop.x as i32,
                y: crop.y as i32,
                w: crop.width,
                h: crop.height,
            }),
            content_dims: crop_size,
            annotation: format!("at ({}, {})", crop.x, crop.y),
            show_extension: None,
        });
    }

    // Step 4: Trim (only show when no Crop step — otherwise Trim is an
    // implementation detail of decoder negotiation that repeats the crop info)
    if layout.source_crop.is_none()
        && let Some(trim) = &plan.trim {
            let decoder_dims = Size::new(trim.x + trim.width, trim.y + trim.height);
            let trim_size = Size::new(trim.width, trim.height);
            steps.push(Step {
                label: String::from("Trim"),
                outer: decoder_dims,
                outer_role: OuterRole::ImageDiscard,
                inner: Some(InnerRect {
                    x: trim.x as i32,
                    y: trim.y as i32,
                    w: trim.width,
                    h: trim.height,
                }),
                content_dims: trim_size,
                annotation: if trim.x == 0 && trim.y == 0 {
                    format!("from {}×{} decode", decoder_dims.width, decoder_dims.height)
                } else {
                    format!("offset ({},{}) in decode", trim.x, trim.y)
                },
                show_extension: None,
            });
        }

    // Step 5: Resize (if not identity)
    if !plan.resize_is_identity {
        steps.push(Step {
            label: String::from("Resize"),
            outer: plan.resize_to,
            outer_role: OuterRole::ContentFill,
            inner: None,
            content_dims: plan.resize_to,
            annotation: String::new(),
            show_extension: None,
        });
    }

    // Step 6: Canvas + Placement (if canvas differs from resize_to or placement is non-zero)
    let (px, py) = plan.placement;
    if plan.canvas != plan.resize_to || px != 0 || py != 0 {
        steps.push(Step {
            label: format!("Canvas  {}×{}", plan.canvas.width, plan.canvas.height),
            outer: plan.canvas,
            outer_role: OuterRole::Padding,
            inner: Some(InnerRect {
                x: px,
                y: py,
                w: plan.resize_to.width,
                h: plan.resize_to.height,
            }),
            content_dims: plan.resize_to,
            annotation: format!(
                "place at ({}, {}), bg {}",
                px,
                py,
                format_color(&plan.canvas_color)
            ),
            show_extension: None,
        });
    }

    // Step 7: Edge extension (if content_size set)
    if let Some(content) = plan.content_size {
        steps.push(Step {
            label: format!("Extend  {}×{}", plan.canvas.width, plan.canvas.height),
            outer: plan.canvas,
            outer_role: OuterRole::ContentFill,
            inner: Some(InnerRect {
                x: 0,
                y: 0,
                w: content.width,
                h: content.height,
            }),
            content_dims: content,
            annotation: format!(
                "edges replicated to {}×{}",
                plan.canvas.width, plan.canvas.height
            ),
            show_extension: Some(content),
        });
    }

    // Final output step (always shown)
    let final_size = plan.canvas;
    let last = steps.last().map(|s| &s.label);
    let already_final =
        last.is_some_and(|l| l.starts_with("Canvas") || l.starts_with("Extend") || l == "Resize");
    if !already_final || steps.len() == 1 {
        steps.push(Step {
            label: String::from("Output"),
            outer: final_size,
            outer_role: OuterRole::ContentFill,
            inner: None,
            content_dims: final_size,
            annotation: String::new(),
            show_extension: None,
        });
    }

    steps
}

/// Scale a Size to fit within the panel bounds, with a relative size factor.
///
/// `rel` is 0.0..=1.0 — the panel shrinks proportionally to show that this
/// step produces smaller output than the largest step. Clamped to at least 0.5
/// so the smallest panels remain readable.
fn scale_to_fit(size: Size, rel: f64) -> (f64, f64, f64) {
    let w = size.width as f64;
    let h = size.height as f64;
    if w == 0.0 || h == 0.0 {
        return (1.0, 1.0, 1.0);
    }
    let max_w = MAX_PANEL_W * rel;
    let max_h = MAX_PANEL_H * rel;
    let scale = (max_w / w).min(max_h / h);
    (w * scale, h * scale, scale)
}

/// Compute the pixel area of a Size (for relative sizing).
fn pixel_area(size: Size) -> f64 {
    size.width as f64 * size.height as f64
}

/// Render step panels into a complete SVG document.
fn render_steps(steps: &[Step]) -> String {
    if steps.is_empty() {
        return String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#);
    }

    // Compute relative size for each step.
    // Use the sqrt of area ratio so a 4× area difference shows as ~2× visual shrink.
    // Clamp to 0.55..1.0 so the smallest panels are still clearly readable.
    let max_area = steps
        .iter()
        .map(|s| pixel_area(s.outer))
        .fold(0.0_f64, f64::max);
    let rel_sizes: Vec<f64> = steps
        .iter()
        .map(|s| {
            if max_area <= 0.0 {
                1.0
            } else {
                (pixel_area(s.outer) / max_area).sqrt().clamp(0.55, 1.0)
            }
        })
        .collect();

    // Calculate total height
    let mut total_h = MARGIN_TOP;
    for (i, _step) in steps.iter().enumerate() {
        total_h += LABEL_H;
        total_h += MAX_PANEL_H;
        if i < steps.len() - 1 {
            total_h += PANEL_GAP;
        }
    }
    total_h += MARGIN_TOP; // bottom margin

    let total_w = MAX_PANEL_W + 2.0 * MARGIN_X;

    let mut svg = String::with_capacity(4096);

    // SVG header
    svg.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}">"#,
        total_w as u32, total_h as u32, total_w, total_h
    ));
    svg.push('\n');

    // Style — light/dark mode via prefers-color-scheme
    svg.push_str(
        r##"<style>
  text { font-family: "Consolas", "DejaVu Sans Mono", "Courier New", monospace; }
  .label { font-size: 13px; font-weight: bold; fill: #333; }
  .dim { font-size: 11px; font-weight: bold; fill: #1a4a70; }
  .annotation { font-size: 11px; fill: #666; }
  .content { fill: #6ba3d6; stroke: #2c6faa; stroke-width: 1.5; }
  .content-fill { fill: #6ba3d6; }
  .discard { fill: #c5d5e4; stroke: #9ab0c5; stroke-width: 1; }
  .padding { fill: #fff; stroke: #bbb; stroke-width: 1; }
  .extend-fill { fill: #b8d4ee; stroke: #7baed0; stroke-width: 1; stroke-dasharray: 4,2; }
  .arrow { stroke: #666; stroke-width: 1.5; fill: none; marker-end: url(#arrowhead); }
  .arrowhead { fill: #666; }
  @media (prefers-color-scheme: dark) {
    .label { fill: #e0e0e0; }
    .dim { fill: #9dc4e8; }
    .annotation { fill: #aaa; }
    .content { fill: #3a72a4; stroke: #5a9fd4; }
    .content-fill { fill: #3a72a4; }
    .discard { fill: #2c3d4d; stroke: #4a6070; }
    .padding { fill: #fff; stroke: #666; }
    .extend-fill { fill: #2a4a65; stroke: #4a7a9e; }
    .arrow { stroke: #888; }
    .arrowhead { fill: #888; }
  }
</style>
"##,
    );

    // Arrow marker definition
    svg.push_str(
        r##"<defs>
  <marker id="arrowhead" markerWidth="8" markerHeight="6" refX="8" refY="3" orient="auto">
    <polygon points="0 0, 8 3, 0 6" class="arrowhead"/>
  </marker>
</defs>
"##,
    );

    let mut y = MARGIN_TOP;
    let center_x = total_w / 2.0;

    for (i, (step, &rel)) in steps.iter().zip(rel_sizes.iter()).enumerate() {
        // Label
        svg.push_str(&format!(
            r#"<text x="{}" y="{}" class="label" text-anchor="middle">{}</text>"#,
            center_x,
            y + 14.0,
            escape_xml(&step.label)
        ));
        svg.push('\n');
        y += LABEL_H;

        // Panel
        let (sw, sh, scale) = scale_to_fit(step.outer, rel);
        let panel_x = center_x - sw / 2.0;
        let panel_y = y;

        // Outer box — class depends on what the surrounding area represents
        let outer_class = match step.outer_role {
            OuterRole::ContentFill => "content",
            OuterRole::ImageDiscard => "discard",
            OuterRole::Padding => "padding",
        };

        // For ContentFill with no inner, the outer IS the content box.
        // For ImageDiscard/Padding, draw the outer as background first.
        if step.outer_role == OuterRole::ContentFill
            && step.inner.is_none()
            && step.show_extension.is_none()
        {
            // Single content rect fills the whole panel
            svg.push_str(&format!(
                r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" class="content" rx="2"/>"#,
                panel_x, panel_y, sw, sh
            ));
            svg.push('\n');

            // Dimension text centered in content
            let dims = format!("{}×{}", step.content_dims.width, step.content_dims.height);
            svg.push_str(&format!(
                r#"<text x="{:.1}" y="{:.1}" class="dim" text-anchor="middle" dominant-baseline="central">{}</text>"#,
                panel_x + sw / 2.0,
                panel_y + sh / 2.0,
                dims
            ));
            svg.push('\n');
        } else {
            // Draw outer background
            svg.push_str(&format!(
                r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" class="{}" rx="2"/>"#,
                panel_x, panel_y, sw, sh, outer_class
            ));
            svg.push('\n');

            // Inner rect (crop region / placed content / extension)
            if let Some(inner) = &step.inner {
                let ix = panel_x + inner.x as f64 * scale;
                let iy = panel_y + inner.y as f64 * scale;
                let iw = inner.w as f64 * scale;
                let ih = inner.h as f64 * scale;

                if let Some(content) = step.show_extension {
                    // Extension panel: content area + dashed extension areas
                    let cw = content.width as f64 * scale;
                    let ch = content.height as f64 * scale;

                    // Content area
                    svg.push_str(&format!(
                        r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" class="content-fill"/>"#,
                        panel_x, panel_y, cw, ch
                    ));
                    svg.push('\n');

                    // Right extension
                    if cw < sw {
                        svg.push_str(&format!(
                            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" class="extend-fill"/>"#,
                            panel_x + cw, panel_y, sw - cw, ch
                        ));
                        svg.push('\n');
                    }

                    // Bottom extension (full width)
                    if ch < sh {
                        svg.push_str(&format!(
                            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" class="extend-fill"/>"#,
                            panel_x, panel_y + ch, sw, sh - ch
                        ));
                        svg.push('\n');
                    }

                    // Dimension text centered in content area
                    let dims = format!("{}×{}", content.width, content.height);
                    svg.push_str(&format!(
                        r#"<text x="{:.1}" y="{:.1}" class="dim" text-anchor="middle" dominant-baseline="central">{}</text>"#,
                        panel_x + cw / 2.0,
                        panel_y + ch / 2.0,
                        dims
                    ));
                    svg.push('\n');
                } else {
                    // Normal inner rect (crop highlight or placed content)
                    svg.push_str(&format!(
                        r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" class="content" rx="1"/>"#,
                        ix, iy, iw, ih
                    ));
                    svg.push('\n');

                    // Dimension text centered in inner rect
                    let dims = format!("{}×{}", step.content_dims.width, step.content_dims.height);
                    svg.push_str(&format!(
                        r#"<text x="{:.1}" y="{:.1}" class="dim" text-anchor="middle" dominant-baseline="central">{}</text>"#,
                        ix + iw / 2.0,
                        iy + ih / 2.0,
                        dims
                    ));
                    svg.push('\n');
                }
            }
        }

        // Annotation
        if !step.annotation.is_empty() {
            svg.push_str(&format!(
                r#"<text x="{}" y="{:.1}" class="annotation" text-anchor="middle">{}</text>"#,
                center_x,
                panel_y + sh + 14.0,
                escape_xml(&step.annotation)
            ));
            svg.push('\n');
        }

        y += MAX_PANEL_H;

        // Arrow to next step
        if i < steps.len() - 1 {
            let arrow_top = y + 8.0;
            let arrow_bot = y + PANEL_GAP - 8.0;
            svg.push_str(&format!(
                r#"<line x1="{}" y1="{:.1}" x2="{}" y2="{:.1}" class="arrow"/>"#,
                center_x, arrow_top, center_x, arrow_bot
            ));
            svg.push('\n');
            y += PANEL_GAP;
        }
    }

    svg.push_str("</svg>\n");
    svg
}

/// Format a CanvasColor concisely for annotations.
fn format_color(color: &crate::constraint::CanvasColor) -> String {
    use crate::constraint::CanvasColor;
    match color {
        CanvasColor::Transparent => String::from("transparent"),
        CanvasColor::Srgb {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        } => String::from("black"),
        CanvasColor::Srgb {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        } => String::from("white"),
        CanvasColor::Srgb { r, g, b, a } => {
            if *a == 255 {
                format!("#{r:02x}{g:02x}{b:02x}")
            } else {
                format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
            }
        }
        CanvasColor::Linear { r, g, b, a } => {
            format!("linear({r:.2},{g:.2},{b:.2},{a:.2})")
        }
    }
}

/// Escape special characters for XML text content.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{DecoderOffer, Pipeline};

    #[test]
    fn svg_identity_passthrough() {
        let (ideal, req) = Pipeline::new(100, 100).plan().unwrap();
        let offer = DecoderOffer::full_decode(100, 100);
        let plan = ideal.finalize(&req, &offer);

        let svg = render_layout_svg(&ideal, &plan);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("100×100"));
        assert!(svg.ends_with("</svg>\n"));
    }

    #[test]
    fn svg_crop_and_resize() {
        let (ideal, req) = Pipeline::new(1000, 800)
            .crop_pixels(100, 100, 600, 400)
            .fit(300, 200)
            .plan()
            .unwrap();

        let offer = DecoderOffer::full_decode(1000, 800);
        let plan = ideal.finalize(&req, &offer);

        let svg = render_layout_svg(&ideal, &plan);
        assert!(svg.contains("Source"));
        assert!(svg.contains("1000×800"));
        assert!(svg.contains("Crop"));
        assert!(svg.contains("600×400"));
        assert!(svg.contains("Resize"));
        assert!(svg.contains("300×200"));
    }

    #[test]
    fn svg_fit_pad_shows_canvas() {
        let (ideal, req) = Pipeline::new(200, 100).fit_pad(100, 100).plan().unwrap();

        let offer = DecoderOffer::full_decode(200, 100);
        let plan = ideal.finalize(&req, &offer);

        assert_eq!(plan.canvas, Size::new(100, 100));
        assert_eq!(plan.resize_to, Size::new(100, 50));

        let svg = render_layout_svg(&ideal, &plan);
        assert!(svg.contains("Canvas"));
        assert!(svg.contains("100×100"));
    }

    #[test]
    fn svg_orientation_shows_orient_step() {
        let (ideal, req) = Pipeline::new(600, 400)
            .auto_orient(6) // Rotate90 → 400×600
            .fit(200, 300)
            .plan()
            .unwrap();

        let offer = DecoderOffer::full_decode(600, 400);
        let plan = ideal.finalize(&req, &offer);

        let svg = render_layout_svg(&ideal, &plan);
        assert!(svg.contains("Orient"));
        assert!(svg.contains("Rotate90"));
    }

    #[test]
    fn svg_output_matches_plan_canvas() {
        let (ideal, req) = Pipeline::new(4000, 3000)
            .auto_orient(6)
            .crop_pixels(200, 200, 3000, 2000)
            .fit_pad(800, 800)
            .plan()
            .unwrap();

        let offer = DecoderOffer::full_decode(4000, 3000);
        let plan = ideal.finalize(&req, &offer);

        let svg = render_layout_svg(&ideal, &plan);

        let final_dim = format!("{}×{}", plan.canvas.width, plan.canvas.height);
        assert!(
            svg.contains(&final_dim),
            "SVG must show final output dimensions {final_dim}"
        );
    }

    #[test]
    fn svg_extend_shows_content_area() {
        use crate::plan::{Align, OutputLimits};

        let (ideal, req) = Pipeline::new(801, 601)
            .output_limits(OutputLimits {
                max: None,
                min: None,
                align: Some(Align::uniform_extend(16)),
            })
            .plan()
            .unwrap();

        let offer = DecoderOffer::full_decode(801, 601);
        let plan = ideal.finalize(&req, &offer);

        assert!(plan.content_size.is_some());
        let svg = render_layout_svg(&ideal, &plan);
        assert!(svg.contains("Extend"));
        assert!(svg.contains("801×601"));
    }

    #[test]
    fn svg_discard_class_for_crop() {
        let (ideal, req) = Pipeline::new(800, 600)
            .crop_pixels(100, 100, 400, 300)
            .plan()
            .unwrap();

        let offer = DecoderOffer::full_decode(800, 600);
        let plan = ideal.finalize(&req, &offer);

        let svg = render_layout_svg(&ideal, &plan);
        assert!(
            svg.contains("class=\"discard\""),
            "Crop outer should use discard class"
        );
    }

    #[test]
    fn svg_padding_class_for_canvas() {
        let (ideal, req) = Pipeline::new(200, 100).fit_pad(100, 100).plan().unwrap();

        let offer = DecoderOffer::full_decode(200, 100);
        let plan = ideal.finalize(&req, &offer);

        let svg = render_layout_svg(&ideal, &plan);
        assert!(
            svg.contains("class=\"padding\""),
            "Canvas outer should use padding class"
        );
    }

    #[test]
    #[ignore] // run with: cargo test --features svg -- --ignored generate_sample_svgs --nocapture
    fn generate_sample_svgs() {
        use crate::constraint::CanvasColor;
        use crate::plan::{Align, OutputLimits, Region, RegionCoord};
        let out = std::env::var("ZENLAYOUT_OUTPUT_DIR")
            .unwrap_or_else(|_| {
                std::env::var("OUTPUT_DIR")
                    .map(|d| format!("{d}/zenlayout/svg"))
                    .unwrap_or_else(|_| {
                        let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/output/svg");
                        target.to_string_lossy().into_owned()
                    })
            });
        let doc = concat!(env!("CARGO_MANIFEST_DIR"), "/doc/svg");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::create_dir_all(doc).unwrap();

        let cases: Vec<(&str, String)> = vec![
            // 1. Simple resize (most common operation)
            {
                let (ideal, req) = Pipeline::new(4000, 3000).fit(800, 600).plan().unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(4000, 3000));
                ("fit", render_layout_svg(&ideal, &plan))
            },
            // 2. FitCrop — crop to different aspect ratio
            {
                let (ideal, req) = Pipeline::new(1920, 1080).fit_crop(500, 500).plan().unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(1920, 1080));
                ("fit_crop", render_layout_svg(&ideal, &plan))
            },
            // 3. FitPad — letterbox into a square
            {
                let (ideal, req) = Pipeline::new(1600, 900).fit_pad(400, 400).plan().unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(1600, 900));
                ("fit_pad", render_layout_svg(&ideal, &plan))
            },
            // 4. Explicit crop + resize
            {
                let (ideal, req) = Pipeline::new(1000, 800)
                    .crop_pixels(100, 50, 600, 500)
                    .fit(300, 250)
                    .plan()
                    .unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(1000, 800));
                ("crop_resize", render_layout_svg(&ideal, &plan))
            },
            // 5. EXIF orientation + resize
            {
                let (ideal, req) = Pipeline::new(4000, 3000)
                    .auto_orient(6)
                    .fit(600, 800)
                    .plan()
                    .unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(4000, 3000));
                ("orient_resize", render_layout_svg(&ideal, &plan))
            },
            // 6. Full pipeline — orient + crop + fit_pad
            {
                let (ideal, req) = Pipeline::new(4000, 3000)
                    .auto_orient(6)
                    .crop_pixels(200, 200, 2600, 2600)
                    .fit_pad(800, 800)
                    .plan()
                    .unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(4000, 3000));
                ("orient_crop_pad", render_layout_svg(&ideal, &plan))
            },
            // 7. MCU edge extension
            {
                let (ideal, req) = Pipeline::new(801, 601)
                    .output_limits(OutputLimits {
                        max: None,
                        min: None,
                        align: Some(Align::uniform_extend(16)),
                    })
                    .plan()
                    .unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(801, 601));
                ("mcu_extend", render_layout_svg(&ideal, &plan))
            },
            // 8. WithinCrop — downscale only, crop to target ratio
            {
                let (ideal, req) = Pipeline::new(800, 600)
                    .within_crop(400, 400)
                    .plan()
                    .unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(800, 600));
                ("within_crop", render_layout_svg(&ideal, &plan))
            },
            // 9. Region viewport — mixed crop + pad
            {
                let (ideal, req) = Pipeline::new(800, 600)
                    .region_viewport(-50, 0, 600, 600, CanvasColor::black())
                    .plan()
                    .unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(800, 600));
                ("region_viewport", render_layout_svg(&ideal, &plan))
            },
            // 10. Region pad — uniform padding
            {
                let (ideal, req) = Pipeline::new(800, 600)
                    .region_pad(50, CanvasColor::white())
                    .fit(450, 350)
                    .plan()
                    .unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(800, 600));
                ("region_pad", render_layout_svg(&ideal, &plan))
            },
            // 11. Region percentage crop
            {
                let reg = Region {
                    left: RegionCoord::pct(0.1),
                    top: RegionCoord::pct(0.1),
                    right: RegionCoord::pct(0.9),
                    bottom: RegionCoord::pct(0.9),
                    color: CanvasColor::Transparent,
                };
                let (ideal, req) = Pipeline::new(1000, 500)
                    .region(reg)
                    .fit(400, 200)
                    .plan()
                    .unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(1000, 500));
                ("region_pct_crop", render_layout_svg(&ideal, &plan))
            },
            // 12. Rotate 90 + resize
            {
                let (ideal, req) = Pipeline::new(1920, 1080)
                    .rotate_90()
                    .fit(540, 960)
                    .plan()
                    .unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(1920, 1080));
                ("rotate_90", render_layout_svg(&ideal, &plan))
            },
            // 13. Rotate 180 (no dimension swap)
            {
                let (ideal, req) = Pipeline::new(800, 600)
                    .rotate_180()
                    .fit(400, 300)
                    .plan()
                    .unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(800, 600));
                ("rotate_180", render_layout_svg(&ideal, &plan))
            },
            // 14. Flip horizontal + resize
            {
                let (ideal, req) = Pipeline::new(800, 600)
                    .flip_h()
                    .fit(400, 300)
                    .plan()
                    .unwrap();
                let plan = ideal.finalize(&req, &DecoderOffer::full_decode(800, 600));
                ("flip_h", render_layout_svg(&ideal, &plan))
            },
        ];

        for (name, svg) in &cases {
            std::fs::write(format!("{out}/{name}.svg"), svg).unwrap();
            std::fs::write(format!("{doc}/{name}.svg"), svg).unwrap();
        }

        println!("Generated {} SVGs in {out} and {doc}", cases.len());
    }

    #[test]
    fn svg_is_valid_xml() {
        let (ideal, req) = Pipeline::new(1920, 1080)
            .auto_orient(3) // Rotate180
            .crop_percent(0.1, 0.1, 0.8, 0.8)
            .within_crop(800, 600)
            .plan()
            .unwrap();

        let offer = DecoderOffer::full_decode(1920, 1080);
        let plan = ideal.finalize(&req, &offer);

        let svg = render_layout_svg(&ideal, &plan);

        // Basic XML validity checks
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(!svg.contains("<<"));
    }
}
