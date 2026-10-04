//! Trigram candidate selection with exact byte-safe matching.
use anyhow::Result;
pub use fs_core::native::{directory_counts, is_native};
use fs_core::{config::IndexConfig, native::grams, platform::normalize, query::SearchOptions};

pub struct Query {
    matcher: crate::matching::Query,
    grams: Vec<u32>,
    basename: bool,
}

impl Query {
    pub fn new(options: &SearchOptions) -> Result<Self> {
        let matcher = crate::matching::Query::new(options)?;
        let mut candidates = Vec::new();
        // Regex alternatives/quantifiers need a full scan. For globs, only
        // literal runs outside character classes are mandatory.
        if !options.regex {
            for pattern in &options.patterns {
                let mut run = Vec::new();
                let mut in_class = false;
                for &byte in normalize(pattern.as_encoded_bytes()).iter() {
                    if byte == b'[' {
                        candidates.extend(grams(&run));
                        run.clear();
                        in_class = true;
                    } else if byte == b']' && in_class {
                        in_class = false;
                    } else if in_class {
                        continue;
                    } else if matches!(byte, b'*' | b'?') {
                        candidates.extend(grams(&run));
                        run.clear();
                    } else {
                        run.push(byte);
                    }
                }
                candidates.extend(grams(&run));
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        Ok(Self {
            matcher,
            grams: candidates,
            basename: options.basename,
        })
    }

    pub fn matches(&self, path: &[u8]) -> bool {
        self.matcher.matches(path)
    }
}

pub fn visit(
    idx: &IndexConfig,
    query: &Query,
    visitor: impl FnMut(&[u8]) -> Result<bool>,
) -> Result<bool> {
    fs_core::native::visit(
        idx,
        &query.grams,
        query.basename,
        |path| query.matches(path),
        visitor,
    )
}

/// Use bounded workers for large, unindexed scans; small scans stay serial.
pub fn visit_filtered_parallel(
    idx: &IndexConfig,
    query: &Query,
    name_matches: impl Fn(&[u8]) -> bool + Sync,
    visitor: impl FnMut(&[u8]) -> Result<bool>,
) -> Result<bool> {
    // Only full scans need workers. Leave one core for SQLite/output;
    // the scan module owns the bounds and serial warmup.
    let workers = if query.grams.is_empty() {
        std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1))
    } else {
        1
    };
    fs_core::native::visit_filtered_parallel(
        idx,
        &query.grams,
        query.basename,
        |path| query.matches(path),
        name_matches,
        visitor,
        workers,
    )
}
