// Copied from imazen/zencodec f5a7fd46231048a7290f11df5b32e010a3e529a8,
// perf/io-validation/src/transport_model.rs, itself reused from imazen/zqi
// b3856304ed60ee232b792a18460aadaf3886a63e (Imazen-owned, MIT OR Apache-2.0).
// Only change: Arc<Mutex<_>> became Arc<Mutex<_>> so a zenpipe source (Send) can own it.
#![allow(dead_code)]
//! Deterministic network-cost model around the real positional-read interface.
//! Cached extents model retained encoded bytes; the immutable backing source
//! still supplies the actual bytes. This is not a process-memory measurement.
// Reused from imazen/zqi b3856304ed60ee232b792a18460aadaf3886a63e,
// imij-set/examples/access_model/transport.rs (MIT OR Apache-2.0).
// Only trait/error adaptation changes the transport model.
use zencodec::source::ReadAt;
#[derive(Debug)]
pub enum Error {
    Read,
    InvalidInput(&'static str),
    Source(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
use std::{
    ops::Range,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub ranges: bool,
    pub cache_bytes: u64,
    pub latency_ms: f64,
    pub bytes_per_ms: f64,
    pub concurrency: usize,
    pub merge_gap: u64,
    pub max_range: u64,
    /// Zero is one object; otherwise physical part size (shared logical cache budget).
    pub part_size: u64,
    pub shared_cache: bool,
}
impl Config {
    pub fn validate(self) -> Result<Self, &'static str> {
        if !self.latency_ms.is_finite()
            || self.latency_ms < 0.0
            || !self.bytes_per_ms.is_finite()
            || self.bytes_per_ms <= 0.0
            || self.concurrency == 0
            || self.max_range == 0
        {
            return Err("invalid transport model");
        }
        Ok(self)
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    pub object: String,
    pub round: usize,
    pub start_ms: f64,
    pub end_ms: f64,
    pub ranges: Vec<Range<u64>>,
    pub status: u16,
    pub logical_base: u64,
    pub object_length: u64,
}
#[derive(Default, Debug)]
pub struct Ledger {
    pub now_ms: f64,
    pub rounds: usize,
    pub requested_bytes: u64,
    pub cache_hit_bytes: u64,
    pub transferred_bytes: u64,
    pub events: Vec<Event>,
    cache: Arc<Mutex<Cache>>,
    cache_budget: Option<u64>,
}
pub type Shared = Arc<Mutex<Ledger>>;

/// Multiple real readers and a payload batcher share one object/cache identity.
pub struct SharedSource<S>(Arc<Mutex<Transport<S>>>);
impl<S> Clone for SharedSource<S> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<S> SharedSource<S> {
    pub fn new(source: Transport<S>) -> Self {
        Self(Arc::new(Mutex::new(source)))
    }
}
impl<S: ReadAt> ReadAt for SharedSource<S> {
    type Error = Error;
    fn len(&mut self) -> Result<u64, Error> {
        self.0.lock().unwrap().len()
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.0.lock().unwrap().read_exact_at(offset, out)
    }
    fn read_many_at(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        self.0.lock().unwrap().read_many_at(requests)
    }
}
#[derive(Clone, Debug)]
struct Extent {
    object: String,
    range: Range<u64>,
    used: u64,
}

#[derive(Default, Debug)]
struct Cache {
    extents: Vec<Extent>,
    tick: u64,
}

pub struct Transport<S> {
    source: S,
    length: u64,
    name: String,
    cfg: Config,
    pub ledger: Shared,
    cache: Arc<Mutex<Cache>>,
}
impl<S: ReadAt> Transport<S> {
    pub fn new(mut source: S, name: String, cfg: Config, ledger: Shared) -> Result<Self, Error> {
        cfg.validate().map_err(Error::InvalidInput)?;
        let length = source.len().map_err(|e| Error::Source(e.to_string()))?;
        let cache = if cfg.shared_cache {
            let mut l = ledger.lock().unwrap();
            if l.cache_budget
                .is_some_and(|budget| budget != cfg.cache_bytes)
            {
                return Err(Error::InvalidInput("shared cache budget changed"));
            }
            l.cache_budget = Some(cfg.cache_bytes);
            l.cache.clone()
        } else {
            Arc::new(Mutex::new(Cache::default()))
        };
        Ok(Self {
            source,
            length,
            name,
            cfg,
            ledger,
            cache,
        })
    }
    pub fn retained(&self) -> u64 {
        self.cache
            .lock()
            .unwrap()
            .extents
            .iter()
            .map(|e| e.range.end - e.range.start)
            .sum()
    }
    fn missing(&mut self, range: Range<u64>) -> Vec<Range<u64>> {
        let mut cursor = range.start;
        let mut missing = Vec::new();
        let mut cache = self.cache.lock().unwrap();
        cache.tick += 1;
        let tick = cache.tick;
        for e in &mut cache.extents {
            if e.object != self.name {
                continue;
            }
            if e.range.end <= cursor || e.range.start >= range.end {
                continue;
            }
            if e.range.start > cursor {
                missing.push(cursor..e.range.start);
            }
            cursor = cursor.max(e.range.end.min(range.end));
            e.used = tick;
        }
        if cursor < range.end {
            missing.push(cursor..range.end);
        }
        missing
    }
    fn retain(&mut self, range: Range<u64>) {
        if range.end - range.start > self.cfg.cache_bytes {
            return;
        }
        let mut cache = self.cache.lock().unwrap();
        cache.tick += 1;
        let tick = cache.tick;
        // Split overlapping old extents; avoid both double accounting and
        // eviction of a recently read subrange as part of an older extent.
        let mut next = Vec::new();
        for e in cache.extents.drain(..) {
            if e.object != self.name || e.range.end <= range.start || e.range.start >= range.end {
                next.push(e);
                continue;
            }
            if e.range.start < range.start {
                next.push(Extent {
                    object: e.object.clone(),
                    range: e.range.start..range.start,
                    used: e.used,
                });
            }
            if e.range.end > range.end {
                next.push(Extent {
                    object: e.object.clone(),
                    range: range.end..e.range.end,
                    used: e.used,
                });
            }
        }
        next.push(Extent {
            object: self.name.clone(),
            range,
            used: tick,
        });
        cache.extents = next;
        while cache
            .extents
            .iter()
            .map(|e| e.range.end - e.range.start)
            .sum::<u64>()
            > self.cfg.cache_bytes
            || cache.extents.len() > 4096
        {
            let i = cache
                .extents
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.used)
                .unwrap()
                .0;
            cache.extents.remove(i);
        }
        cache
            .extents
            .sort_by(|a, b| (&a.object, a.range.start).cmp(&(&b.object, b.range.start)));
        drop(cache);
        debug_assert!(self.retained() <= self.cfg.cache_bytes);
    }
    fn request(&mut self, ranges: Vec<Range<u64>>) -> Result<(), Error> {
        let mut missing = Vec::new();
        for range in ranges {
            if range.start > range.end || range.end > self.length {
                return Err(Error::Read);
            }
            let requested = range.end - range.start;
            let holes = self.missing(range);
            let bytes: u64 = holes.iter().map(|r| r.end - r.start).sum();
            let mut l = self.ledger.lock().unwrap();
            l.requested_bytes += requested;
            l.cache_hit_bytes += requested - bytes;
            missing.extend(holes);
        }
        if missing.is_empty() {
            return Ok(());
        }
        // Split before merging: a gap may never join two physical objects.
        let part_size = if self.cfg.part_size == 0 {
            self.length.max(1)
        } else {
            self.cfg.part_size
        };
        missing = missing
            .into_iter()
            .flat_map(|r| split_parts(r, part_size))
            .collect();
        missing.sort_by_key(|r| r.start);
        let mut merged: Vec<Range<u64>> = Vec::new();
        for r in missing {
            if let Some(last) = merged.last_mut() {
                if r.start / part_size == last.start / part_size
                    && (r.start <= last.end
                        || (r.start <= last.end.saturating_add(self.cfg.merge_gap)
                            && r.end.max(last.end) - last.start <= self.cfg.max_range))
                {
                    last.end = last.end.max(r.end);
                    continue;
                }
                if r.end <= last.end {
                    continue;
                }
            }
            merged.push(r);
        }
        let fetched: Vec<Range<u64>> = if self.cfg.ranges {
            merged
                .into_iter()
                .flat_map(|r| {
                    let mut out = Vec::new();
                    let mut start = r.start;
                    while start < r.end {
                        let end = start.saturating_add(self.cfg.max_range).min(r.end);
                        out.push(start..end);
                        start = end;
                    }
                    out
                })
                .collect()
        } else {
            let mut parts = merged
                .iter()
                .map(|r| r.start / part_size)
                .collect::<Vec<_>>();
            parts.dedup();
            parts
                .into_iter()
                .map(|i| {
                    let start = i * part_size;
                    start..start.saturating_add(part_size).min(self.length)
                })
                .collect()
        };
        let mut l = self.ledger.lock().unwrap();
        l.rounds += 1;
        let round = l.rounds;
        // Independent requests share one aggregate bandwidth budget. Each
        // admission wave pays one RTT plus its aggregate transfer time.
        for wave in fetched.chunks(self.cfg.concurrency) {
            let bytes: u64 = wave.iter().map(|r| r.end - r.start).sum();
            let start_ms = l.now_ms;
            l.now_ms += self.cfg.latency_ms + bytes as f64 / self.cfg.bytes_per_ms;
            let end_ms = l.now_ms;
            l.transferred_bytes += bytes;
            // Same-wave parts share timestamps and aggregate bandwidth, not serial RTTs.
            for range in wave {
                let base = (range.start / part_size) * part_size;
                let object = if self.cfg.part_size == 0 {
                    self.name.clone()
                } else {
                    format!("{}/part-{:06}", self.name, range.start / part_size)
                };
                let local = range.start - base..range.end - base;
                if let Some(last) = l
                    .events
                    .last_mut()
                    .filter(|e| e.round == round && e.start_ms == start_ms && e.object == object)
                {
                    last.ranges.push(local);
                } else {
                    l.events.push(Event {
                        object,
                        round,
                        start_ms,
                        end_ms,
                        ranges: std::iter::once(local).collect(),
                        status: if self.cfg.ranges { 206 } else { 200 },
                        logical_base: base,
                        object_length: self.length.saturating_sub(base).min(part_size),
                    });
                }
            }
        }
        drop(l);
        for range in fetched {
            self.retain(range);
        }
        Ok(())
    }
}
impl<S: ReadAt> ReadAt for Transport<S> {
    type Error = Error;
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.length)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.read_many_at(&mut [(offset, out)])
    }
    fn read_many_at(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        let ranges = requests
            .iter()
            .filter(|(_, b)| !b.is_empty())
            .map(|(o, b)| {
                o.checked_add(b.len() as u64)
                    .map(|e| *o..e)
                    .ok_or(Error::Read)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.source
            .read_many_at(requests)
            .map_err(|e| Error::Source(e.to_string()))?;
        self.request(ranges)
    }
}

fn split_parts(range: Range<u64>, size: u64) -> Vec<Range<u64>> {
    let mut out = Vec::new();
    let mut start = range.start;
    while start < range.end {
        let base = start / size * size;
        let end = base.saturating_add(size).min(range.end);
        out.push(start..end);
        start = end;
    }
    out
}
