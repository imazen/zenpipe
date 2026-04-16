//! Resource limits and metadata types.

/// Resource limits for decode/encode operations.
///
/// Used to prevent DoS attacks and resource exhaustion. All limits are optional.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Maximum image width in pixels.
    pub max_width: Option<u64>,
    /// Maximum image height in pixels.
    pub max_height: Option<u64>,
    /// Maximum total pixels (width × height).
    pub max_pixels: Option<u64>,
    /// Maximum memory allocation in bytes.
    pub max_memory_bytes: Option<u64>,
    /// Maximum input data size in bytes (decode only).
    pub max_input_bytes: Option<u64>,
    /// Maximum encoded output size in bytes (encode only).
    pub max_output_bytes: Option<u64>,
    /// Maximum number of animation frames.
    pub max_frames: Option<u32>,
    /// Maximum total animation duration in milliseconds.
    pub max_duration_ms: Option<u64>,
    /// Threading policy for codec operations.
    ///
    /// Defaults to [`ThreadingPolicy::Parallel`]. Use [`ThreadingPolicy::Sequential`]
    /// for deterministic output or constrained environments.
    pub threading: zencodec::ThreadingPolicy,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_width: None,
            max_height: None,
            max_pixels: None,
            max_memory_bytes: None,
            max_input_bytes: None,
            max_output_bytes: None,
            max_frames: None,
            max_duration_ms: None,
            threading: zencodec::ThreadingPolicy::Parallel,
        }
    }
}

impl Limits {
    /// Create a new Limits with no restrictions.
    pub fn none() -> Self {
        Self::default()
    }

    /// Production-safe limits for real-time image proxies processing untrusted input.
    ///
    /// 16384x16384 max dimensions, 100 megapixels, 512 MB memory, 100 MB input,
    /// 1000 frames, 60 seconds duration.
    pub fn for_proxy() -> Self {
        Self {
            max_width: Some(16_384),
            max_height: Some(16_384),
            max_pixels: Some(100_000_000),
            max_memory_bytes: Some(512 * 1024 * 1024),
            max_input_bytes: Some(100 * 1024 * 1024),
            max_output_bytes: Some(100 * 1024 * 1024),
            max_frames: Some(1000),
            max_duration_ms: Some(60_000),
            threading: zencodec::ThreadingPolicy::Parallel,
        }
    }

    /// Set maximum image width in pixels.
    pub fn with_max_width(mut self, max: u64) -> Self {
        self.max_width = Some(max);
        self
    }

    /// Set maximum image height in pixels.
    pub fn with_max_height(mut self, max: u64) -> Self {
        self.max_height = Some(max);
        self
    }

    /// Set maximum total pixels (width x height).
    pub fn with_max_pixels(mut self, max: u64) -> Self {
        self.max_pixels = Some(max);
        self
    }

    /// Set maximum memory allocation in bytes.
    pub fn with_max_memory_bytes(mut self, max: u64) -> Self {
        self.max_memory_bytes = Some(max);
        self
    }

    /// Set maximum input data size in bytes (decode only).
    pub fn with_max_input_bytes(mut self, max: u64) -> Self {
        self.max_input_bytes = Some(max);
        self
    }

    /// Set maximum encoded output size in bytes (encode only).
    pub fn with_max_output_bytes(mut self, max: u64) -> Self {
        self.max_output_bytes = Some(max);
        self
    }

    /// Set maximum number of animation frames.
    pub fn with_max_frames(mut self, max: u32) -> Self {
        self.max_frames = Some(max);
        self
    }

    /// Set maximum total animation duration in milliseconds.
    pub fn with_max_duration_ms(mut self, max: u64) -> Self {
        self.max_duration_ms = Some(max);
        self
    }

    /// Set threading policy for codec operations.
    pub fn with_threading(mut self, policy: zencodec::ThreadingPolicy) -> Self {
        self.threading = policy;
        self
    }

    /// Check if dimensions are within limits.
    ///
    /// Returns `Err` with a description if any limit is exceeded.
    pub fn check_dimensions(&self, width: u64, height: u64) -> Result<(), &'static str> {
        if let Some(max_width) = self.max_width
            && width > max_width
        {
            return Err("width exceeds limit");
        }

        if let Some(max_height) = self.max_height
            && height > max_height
        {
            return Err("height exceeds limit");
        }

        if let Some(max_pixels) = self.max_pixels {
            let pixels = width.saturating_mul(height);
            if pixels > max_pixels {
                return Err("pixel count exceeds limit");
            }
        }

        Ok(())
    }

    /// Check if a memory allocation is within limits.
    pub fn check_memory(&self, bytes: u64) -> Result<(), &'static str> {
        if let Some(max_memory) = self.max_memory_bytes
            && bytes > max_memory
        {
            return Err("memory allocation exceeds limit");
        }
        Ok(())
    }
}

/// Get a `&dyn Stop` reference, defaulting to `Unstoppable` if `None`.
#[cfg(feature = "jpeg-ultrahdr")]
pub(crate) fn stop_or_default(stop: &Option<zencodec::StopToken>) -> &dyn Stop {
    match stop {
        Some(s) => s,
        None => &enough::Unstoppable,
    }
}

/// Convert zencodecs [`Limits`] to zencodec [`ResourceLimits`](zencodec::ResourceLimits).
pub(crate) fn to_resource_limits(limits: &Limits) -> zencodec::ResourceLimits {
    let mut rl = zencodec::ResourceLimits::none();
    if let Some(max_w) = limits.max_width {
        rl = rl.with_max_width(max_w.min(u32::MAX as u64) as u32);
    }
    if let Some(max_h) = limits.max_height {
        rl = rl.with_max_height(max_h.min(u32::MAX as u64) as u32);
    }
    if let Some(max_px) = limits.max_pixels {
        rl = rl.with_max_pixels(max_px);
    }
    if let Some(max_mem) = limits.max_memory_bytes {
        rl = rl.with_max_memory(max_mem);
    }
    if let Some(max_in) = limits.max_input_bytes {
        rl = rl.with_max_input_bytes(max_in);
    }
    if let Some(max_out) = limits.max_output_bytes {
        rl = rl.with_max_output(max_out);
    }
    if let Some(max_fr) = limits.max_frames {
        rl = rl.with_max_frames(max_fr);
    }
    if let Some(max_dur) = limits.max_duration_ms {
        rl = rl.with_max_animation_ms(max_dur);
    }
    rl = rl.with_threading(limits.threading);
    rl
}

/// Re-export `Stop` for cooperative cancellation.
///
/// Codecs periodically call `stop.check()` and return `CodecError::Cancelled`
/// if the operation should be cancelled. Use `enough::Unstoppable` when you
/// don't need cancellation (zero-cost).
pub use enough::Stop;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_none() {
        let limits = Limits::none();
        assert!(limits.check_dimensions(u64::MAX, u64::MAX).is_ok());
        assert!(limits.check_memory(u64::MAX).is_ok());
    }

    #[test]
    fn limits_dimensions() {
        let limits = Limits {
            max_width: Some(1000),
            max_height: Some(1000),
            max_pixels: Some(500_000),
            ..Default::default()
        };

        assert!(limits.check_dimensions(1000, 1000).is_err()); // 1M pixels > 500k
        assert!(limits.check_dimensions(500, 500).is_ok()); // 250k pixels
        assert!(limits.check_dimensions(2000, 500).is_err()); // width > 1000
    }

    #[test]
    fn limits_memory() {
        let limits = Limits {
            max_memory_bytes: Some(1_000_000),
            ..Default::default()
        };

        assert!(limits.check_memory(500_000).is_ok());
        assert!(limits.check_memory(2_000_000).is_err());
    }

    #[test]
    fn to_resource_limits_forwards_all_fields() {
        let limits = Limits {
            max_width: Some(1920),
            max_height: Some(1080),
            max_pixels: Some(2_073_600),
            max_memory_bytes: Some(512_000_000),
            max_input_bytes: Some(10_000_000),
            max_output_bytes: Some(5_000_000),
            max_frames: Some(100),
            max_duration_ms: Some(30_000),
            threading: zencodec::ThreadingPolicy::Sequential,
        };

        let rl = to_resource_limits(&limits);

        assert_eq!(rl.max_width, Some(1920));
        assert_eq!(rl.max_height, Some(1080));
        assert_eq!(rl.max_pixels, Some(2_073_600));
        assert_eq!(rl.max_memory_bytes, Some(512_000_000));
        assert_eq!(rl.max_input_bytes, Some(10_000_000));
        assert_eq!(rl.max_output_bytes, Some(5_000_000));
        assert_eq!(rl.max_frames, Some(100));
        assert_eq!(rl.max_animation_ms, Some(30_000));
        assert_eq!(rl.threading, zencodec::ThreadingPolicy::Sequential);
    }

    #[test]
    fn to_resource_limits_none_fields_stay_none() {
        let limits = Limits::none();
        let rl = to_resource_limits(&limits);

        assert_eq!(rl.max_width, None);
        assert_eq!(rl.max_height, None);
        assert_eq!(rl.max_pixels, None);
        assert_eq!(rl.max_memory_bytes, None);
        assert_eq!(rl.max_input_bytes, None);
        assert_eq!(rl.max_output_bytes, None);
        assert_eq!(rl.max_frames, None);
        assert_eq!(rl.max_animation_ms, None);
        assert_eq!(rl.threading, zencodec::ThreadingPolicy::Parallel);
    }
}
