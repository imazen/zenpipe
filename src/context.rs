/// Reusable scratch buffer pool for filter pipeline operations.
///
/// `FilterContext` eliminates per-call heap allocations by maintaining a
/// pool of temporary `Vec<f32>` planes. Neighborhood filters (clarity,
/// sharpen, blur, brilliance, bilateral) need 1-3 temporary planes per
/// `apply()` call — without a context, each call allocates and drops
/// these planes. With a context, planes are borrowed from the pool and
/// returned after use.
///
/// # Usage
///
/// ```
/// use zenfilters::FilterContext;
///
/// // Create once, reuse across many pipeline applications
/// let mut ctx = FilterContext::new();
///
/// // Borrow a zeroed plane for 1024×768 image
/// let n = 1024 * 768;
/// let mut plane = ctx.take_f32(n);
/// assert_eq!(plane.len(), n);
///
/// // Return it to the pool when done
/// ctx.return_f32(plane);
///
/// // Next take reuses the same allocation (no heap alloc)
/// let plane2 = ctx.take_f32(n);
/// ctx.return_f32(plane2);
/// ```
///
/// The context also provides [`take_u8`](FilterContext::take_u8) for
/// byte buffers used in format conversion paths.
use crate::prelude::*;
pub struct FilterContext {
    f32_pool: Vec<Vec<f32>>,
    u8_pool: Vec<Vec<u8>>,
}

impl FilterContext {
    /// Create an empty context. Pools start empty and grow on demand.
    pub fn new() -> Self {
        Self {
            f32_pool: Vec::new(),
            u8_pool: Vec::new(),
        }
    }

    /// Borrow a zeroed `Vec<f32>` of exactly `len` elements.
    ///
    /// If the pool has a vector with sufficient capacity, it is resized
    /// and zeroed. Otherwise a new one is allocated.
    pub fn take_f32(&mut self, len: usize) -> Vec<f32> {
        if let Some(idx) = self.f32_pool.iter().position(|v| v.capacity() >= len) {
            let mut v = self.f32_pool.swap_remove(idx);
            v.resize(len, 0.0);
            v.fill(0.0);
            v
        } else {
            vec![0.0f32; len]
        }
    }

    /// Return a `Vec<f32>` to the pool for later reuse.
    pub fn return_f32(&mut self, v: Vec<f32>) {
        self.f32_pool.push(v);
    }

    /// Borrow a zeroed `Vec<u8>` of exactly `len` bytes.
    pub fn take_u8(&mut self, len: usize) -> Vec<u8> {
        if let Some(idx) = self.u8_pool.iter().position(|v| v.capacity() >= len) {
            let mut v = self.u8_pool.swap_remove(idx);
            v.resize(len, 0);
            v.fill(0);
            v
        } else {
            vec![0u8; len]
        }
    }

    /// Return a `Vec<u8>` to the pool for later reuse.
    pub fn return_u8(&mut self, v: Vec<u8>) {
        self.u8_pool.push(v);
    }

    /// Number of `f32` vectors currently in the pool.
    pub fn f32_pool_size(&self) -> usize {
        self.f32_pool.len()
    }

    /// Number of `u8` vectors currently in the pool.
    pub fn u8_pool_size(&self) -> usize {
        self.u8_pool.len()
    }

    /// Drop all pooled vectors, freeing memory.
    pub fn clear(&mut self) {
        self.f32_pool.clear();
        self.u8_pool.clear();
    }
}

impl Default for FilterContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_returns_zeroed() {
        let mut ctx = FilterContext::new();
        let v = ctx.take_f32(100);
        assert_eq!(v.len(), 100);
        assert!(v.iter().all(|&x| x == 0.0));
        ctx.return_f32(v);
    }

    #[test]
    fn returned_vec_is_reused() {
        let mut ctx = FilterContext::new();
        let v = ctx.take_f32(1000);
        let ptr = v.as_ptr();
        ctx.return_f32(v);

        let v2 = ctx.take_f32(1000);
        assert_eq!(v2.as_ptr(), ptr, "should reuse same allocation");
        ctx.return_f32(v2);
    }

    #[test]
    fn smaller_request_reuses_larger_capacity() {
        let mut ctx = FilterContext::new();
        let v = ctx.take_f32(2000);
        let cap = v.capacity();
        ctx.return_f32(v);

        let v2 = ctx.take_f32(500);
        assert!(v2.capacity() >= cap, "should reuse larger capacity vec");
        assert_eq!(v2.len(), 500);
        ctx.return_f32(v2);
    }

    #[test]
    fn take_u8_works() {
        let mut ctx = FilterContext::new();
        let v = ctx.take_u8(256);
        assert_eq!(v.len(), 256);
        assert!(v.iter().all(|&x| x == 0));
        let ptr = v.as_ptr();
        ctx.return_u8(v);

        let v2 = ctx.take_u8(256);
        assert_eq!(v2.as_ptr(), ptr);
        ctx.return_u8(v2);
    }

    #[test]
    fn clear_frees_all() {
        let mut ctx = FilterContext::new();
        ctx.return_f32(vec![0.0; 100]);
        ctx.return_f32(vec![0.0; 200]);
        ctx.return_u8(vec![0; 300]);
        assert_eq!(ctx.f32_pool_size(), 2);
        assert_eq!(ctx.u8_pool_size(), 1);

        ctx.clear();
        assert_eq!(ctx.f32_pool_size(), 0);
        assert_eq!(ctx.u8_pool_size(), 0);
    }

    #[test]
    fn multiple_takes_without_return() {
        let mut ctx = FilterContext::new();
        let v1 = ctx.take_f32(100);
        let v2 = ctx.take_f32(100);
        // Both are fresh allocations
        assert_ne!(v1.as_ptr(), v2.as_ptr());
        ctx.return_f32(v1);
        ctx.return_f32(v2);
        assert_eq!(ctx.f32_pool_size(), 2);
    }

    #[test]
    fn zeroes_on_reuse() {
        let mut ctx = FilterContext::new();
        let mut v = ctx.take_f32(10);
        v.fill(42.0);
        ctx.return_f32(v);

        let v2 = ctx.take_f32(10);
        assert!(v2.iter().all(|&x| x == 0.0), "must be zeroed on reuse");
        ctx.return_f32(v2);
    }
}
