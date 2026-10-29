//! Orientation passthrough versus rendering, with the buffering each actually
//! needs in a single-pass, top-to-bottom row pipeline.
//!
//! Passthrough keeps the stored rows and carries the orientation in output
//! metadata: no rows are held. Rendering upright pixels is row-local only for
//! `Identity` and `FlipH`. Every other orientation needs the last stored row
//! (reversed rows) or a stored column (axis swap) before its first output row,
//! so a decoder that emits rows once, top to bottom, must have its whole
//! raster held first.

use alloc::boxed::Box;
use alloc::sync::Arc;

use whereat::at;
use zencodec::Orientation;
use zencodec::storage::{OwnedStorage, StorageBudget};
use zenpixels::{ColorContext, PixelDescriptor, PixelSlice};

use super::Refusal;
use crate::error::{PipeError, PipeResult};
use crate::format::PixelFormat;
use crate::sources::FlipHSource;
use crate::{Source, Strip};

/// What rendering `orientation` upright costs a single-pass row pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OrientationCost {
    /// The stored orientation being rendered.
    pub orientation: Orientation,
    /// Upright output width.
    pub width: u32,
    /// Upright output height.
    pub height: u32,
    /// Output rows are stored columns (`Transpose`, `Rotate90`,
    /// `Transverse`, `Rotate270`).
    pub swaps_axes: bool,
    /// Output rows are stored rows in reverse order (`FlipV`, `Rotate180`).
    pub reverses_rows: bool,
    /// Stored rows held before the first upright row exists: zero for
    /// `Identity` and `FlipH`, the whole stored height otherwise.
    pub held_rows: u32,
    /// Bytes for the held rows at the stored descriptor; `None` on overflow.
    pub held_bytes: Option<u64>,
}

impl OrientationCost {
    /// The cost of rendering a `width` x `height` stored raster upright.
    pub fn new(
        orientation: Orientation,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> Self {
        let (out_width, out_height) = orientation.output_dimensions(width, height);
        let held_rows = if orientation.is_row_local() {
            0
        } else {
            height
        };
        let held_bytes = u64::from(width)
            .checked_mul(descriptor.bytes_per_pixel() as u64)
            .and_then(|row| row.checked_mul(u64::from(held_rows)));
        Self {
            orientation,
            width: out_width,
            height: out_height,
            swaps_axes: orientation.swaps_axes(),
            reverses_rows: matches!(orientation, Orientation::FlipV | Orientation::Rotate180),
            held_rows,
            held_bytes,
        }
    }

    /// Whether rendering streams without holding stored rows.
    pub fn streams(&self) -> bool {
        self.held_rows == 0
    }
}

/// Render `source`'s stored rows upright.
///
/// `Identity` returns the source and `FlipH` mirrors each strip as it passes.
/// Every other orientation admits one stored raster from `storage` before
/// reading any row (an insufficient budget refuses with no pixel work), drains
/// the source on the first pull, then serves upright strips of `strip_rows`
/// from the held raster: one raster plus one strip, never a second raster.
pub fn render_orientation(
    source: Box<dyn Source>,
    orientation: Orientation,
    storage: &StorageBudget,
    strip_rows: u32,
) -> PipeResult<Box<dyn Source>> {
    match orientation {
        Orientation::Identity => Ok(source),
        Orientation::FlipH => Ok(Box::new(FlipHSource::new(source))),
        _ => Ok(Box::new(OrientedRaster::new(
            source,
            orientation,
            storage,
            strip_rows,
        )?)),
    }
}

/// The stored pixel shown at upright position (`x`, `y`), for a stored
/// raster `width` x `height`.
fn stored_position(o: Orientation, x: u32, y: u32, width: u32, height: u32) -> (u32, u32) {
    match o {
        Orientation::FlipH => (width - 1 - x, y),
        Orientation::Rotate180 => (width - 1 - x, height - 1 - y),
        Orientation::FlipV => (x, height - 1 - y),
        Orientation::Transpose => (y, x),
        Orientation::Rotate90 => (y, height - 1 - x),
        Orientation::Rotate270 => (width - 1 - y, x),
        Orientation::Transverse => (width - 1 - y, height - 1 - x),
        _ => (x, y),
    }
}

struct OrientedRaster {
    upstream: Option<Box<dyn Source>>,
    raster: OwnedStorage<u8>,
    strip: OwnedStorage<u8>,
    orientation: Orientation,
    stored: (u32, u32),
    upright: (u32, u32),
    bpp: usize,
    format: PixelFormat,
    strip_rows: u32,
    color: Option<Arc<ColorContext>>,
    y: u32,
}

impl OrientedRaster {
    fn new(
        upstream: Box<dyn Source>,
        orientation: Orientation,
        storage: &StorageBudget,
        strip_rows: u32,
    ) -> PipeResult<Self> {
        let (width, height, format) = (upstream.width(), upstream.height(), upstream.format());
        if strip_rows == 0 || width == 0 || height == 0 {
            return Err(at!(PipeError::Codec(Box::new(Refusal::InvalidOptions))));
        }
        let cost = OrientationCost::new(orientation, width, height, format);
        let bpp = format.bytes_per_pixel();
        let held = cost
            .held_bytes
            .and_then(|bytes| usize::try_from(bytes).ok())
            .ok_or_else(|| at!(PipeError::Codec(Box::new(Refusal::StripGeometry))))?;
        let strip_rows = strip_rows.min(cost.height);
        let strip_len = (cost.width as usize)
            .checked_mul(bpp)
            .and_then(|row| row.checked_mul(strip_rows as usize))
            .ok_or_else(|| at!(PipeError::Codec(Box::new(Refusal::StripGeometry))))?;
        let color = upstream.color_context();
        let raster = storage
            .bytes(held)
            .map_err(|e| at!(PipeError::Codec(Box::new(e))))?;
        let strip = storage
            .bytes(strip_len)
            .map_err(|e| at!(PipeError::Codec(Box::new(e))))?;
        Ok(Self {
            upstream: Some(upstream),
            raster,
            strip,
            orientation,
            stored: (width, height),
            upright: (cost.width, cost.height),
            bpp,
            format,
            strip_rows,
            color,
            y: 0,
        })
    }

    fn drain(&mut self) -> PipeResult<()> {
        let Some(mut upstream) = self.upstream.take() else {
            return Ok(());
        };
        let row_bytes = self.stored.0 as usize * self.bpp;
        let mut y = 0usize;
        while let Some(strip) = upstream.next()? {
            if strip.width() != self.stored.0 || strip.descriptor() != self.format {
                return Err(at!(PipeError::Codec(Box::new(Refusal::SourceChanged))));
            }
            if let (Some(context), Some(declared)) = (strip.color_context(), &self.color)
                && context != declared
            {
                return Err(at!(PipeError::Codec(Box::new(Refusal::ColorChanged))));
            }
            for r in 0..strip.rows() {
                let start = y * row_bytes;
                let dst = self
                    .raster
                    .as_mut_slice()
                    .get_mut(start..start + row_bytes)
                    .ok_or_else(|| at!(PipeError::Codec(Box::new(Refusal::StripGeometry))))?;
                dst.copy_from_slice(strip.row(r));
                y += 1;
            }
        }
        if y != self.stored.1 as usize {
            return Err(at!(PipeError::Codec(Box::new(Refusal::MissingRows))));
        }
        Ok(())
    }
}

impl Source for OrientedRaster {
    fn next(&mut self) -> PipeResult<Option<Strip<'_>>> {
        self.drain()?;
        let (width, height) = self.upright;
        if self.y >= height {
            return Ok(None);
        }
        let rows = self.strip_rows.min(height - self.y);
        let bpp = self.bpp;
        let row_bytes = width as usize * bpp;
        let stored_row = self.stored.0 as usize * bpp;
        let raster = self.raster.as_slice();
        let out = self.strip.as_mut_slice();
        for r in 0..rows {
            let y = self.y + r;
            let line = &mut out[r as usize * row_bytes..(r as usize + 1) * row_bytes];
            for x in 0..width {
                let (sx, sy) =
                    stored_position(self.orientation, x, y, self.stored.0, self.stored.1);
                let src = sy as usize * stored_row + sx as usize * bpp;
                line[x as usize * bpp..(x as usize + 1) * bpp]
                    .copy_from_slice(&raster[src..src + bpp]);
            }
        }
        self.y += rows;
        let mut strip = PixelSlice::new(
            &self.strip.as_slice()[..rows as usize * row_bytes],
            width,
            rows,
            row_bytes,
            self.format,
        )
        .map_err(|e| at!(PipeError::Codec(Box::new(e))))?;
        if let Some(color) = &self.color {
            strip = strip.with_color_context(color.clone());
        }
        Ok(Some(strip))
    }

    fn width(&self) -> u32 {
        self.upright.0
    }

    fn height(&self) -> u32 {
        self.upright.1
    }

    fn format(&self) -> PixelFormat {
        self.format
    }

    /// Reorienting keeps the stored samples' interpretation.
    fn color_context(&self) -> Option<Arc<ColorContext>> {
        self.color.clone()
    }
}
