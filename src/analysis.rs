//! Typed analysis outputs collected during pipeline execution.
//!
//! [`AnalysisOutputs`] is a heterogeneous bag that analysis nodes
//! (face detection, saliency, classification) write to during pipeline
//! execution. The caller retrieves typed results after the pipeline completes.
//!
//! # Example
//!
//! ```ignore
//! // Node crate (zenfaces) defines its output type:
//! #[derive(Debug, serde::Serialize)]
//! pub struct FaceDetection { pub x: u32, pub y: u32, pub w: u32, pub h: u32, pub confidence: f32 }
//!
//! // Bridge converter creates an Analyze node that writes to outputs:
//! let op = NodeOp::Analyze(Box::new(|mat, outputs| {
//!     let faces = zenfaces::detect(&mat);
//!     outputs.insert("zenfaces", faces);
//!     Ok(Box::new(mat))
//! }));
//!
//! // Caller retrieves typed results:
//! let result = zenpipe::process(source, &config)?;
//! if let Some(faces) = result.outputs.get::<Vec<FaceDetection>>("zenfaces") {
//!     println!("Found {} faces", faces.len());
//! }
//! ```

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;

/// A typed collection of analysis results produced during pipeline execution.
///
/// Keyed by static string identifiers (conventionally the crate name, e.g.,
/// `"zenfaces"`, `"zensally"`). Values are type-erased via [`Any`] — callers
/// downcast to the concrete type defined by the analysis node crate.
pub struct AnalysisOutputs {
    entries: hashbrown::HashMap<&'static str, Box<dyn Any + Send>>,
}

impl Default for AnalysisOutputs {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for AnalysisOutputs {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AnalysisOutputs")
            .field("keys", &self.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl AnalysisOutputs {
    /// Create an empty output bag.
    pub fn new() -> Self {
        Self {
            entries: hashbrown::HashMap::new(),
        }
    }

    /// Insert a typed analysis result.
    ///
    /// If a value with the same key already exists, it is replaced.
    pub fn insert<T: Any + Send>(&mut self, key: &'static str, value: T) {
        self.entries.insert(key, Box::new(value));
    }

    /// Retrieve a typed analysis result by key.
    ///
    /// Returns `None` if the key is missing or the type doesn't match.
    pub fn get<T: Any + Send>(&self, key: &str) -> Option<&T> {
        self.entries.get(key).and_then(|v| v.downcast_ref())
    }

    /// Remove and return a typed analysis result.
    ///
    /// Returns `None` if the key is missing or the type doesn't match.
    pub fn remove<T: Any + Send>(&mut self, key: &str) -> Option<T> {
        let boxed = self.entries.remove(key)?;
        boxed.downcast().ok().map(|b| *b)
    }

    /// Iterate over the keys of all stored outputs.
    pub fn keys(&self) -> impl Iterator<Item = &&'static str> {
        self.entries.keys()
    }

    /// Number of stored outputs.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no outputs have been stored.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// List all keys as owned strings (useful for serialization).
    pub fn key_list(&self) -> Vec<String> {
        self.entries.keys().map(|k| String::from(*k)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_get() {
        let mut outputs = AnalysisOutputs::new();
        outputs.insert("faces", vec![(10u32, 20u32, 50u32, 50u32)]);
        let faces = outputs.get::<Vec<(u32, u32, u32, u32)>>("faces").unwrap();
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0], (10, 20, 50, 50));
    }

    #[test]
    fn get_wrong_type_returns_none() {
        let mut outputs = AnalysisOutputs::new();
        outputs.insert("faces", 42u32);
        assert!(outputs.get::<String>("faces").is_none());
    }

    #[test]
    fn get_missing_key_returns_none() {
        let outputs = AnalysisOutputs::new();
        assert!(outputs.get::<u32>("missing").is_none());
    }

    #[test]
    fn remove_returns_value() {
        let mut outputs = AnalysisOutputs::new();
        outputs.insert("score", 0.95f64);
        let score = outputs.remove::<f64>("score").unwrap();
        assert!((score - 0.95).abs() < f64::EPSILON);
        assert!(outputs.is_empty());
    }

    #[test]
    fn keys_and_len() {
        let mut outputs = AnalysisOutputs::new();
        assert!(outputs.is_empty());
        outputs.insert("a", 1u32);
        outputs.insert("b", 2u32);
        assert_eq!(outputs.len(), 2);
        let mut keys: Vec<_> = outputs.keys().copied().collect();
        keys.sort();
        assert_eq!(keys, vec!["a", "b"]);
    }

    #[test]
    fn key_list() {
        let mut outputs = AnalysisOutputs::new();
        outputs.insert("zenfaces", true);
        let list = outputs.key_list();
        assert_eq!(list, vec!["zenfaces"]);
    }

    #[test]
    fn replace_existing_key() {
        let mut outputs = AnalysisOutputs::new();
        outputs.insert("v", 1u32);
        outputs.insert("v", 2u32);
        assert_eq!(*outputs.get::<u32>("v").unwrap(), 2);
    }

    #[test]
    fn debug_output() {
        let mut outputs = AnalysisOutputs::new();
        outputs.insert("test", 42u32);
        let debug = alloc::format!("{outputs:?}");
        assert!(debug.contains("AnalysisOutputs"));
        assert!(debug.contains("test"));
    }
}
