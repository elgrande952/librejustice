//! Chars-to-tokens calibration of the reference tokenizer (Qwen3-Embedding-0.6B).
//!
//! Empirical constants (2,700 decisions) shared by every token estimate made
//! from a length in characters: the visa-trim budget of `lj-core` and the
//! Cloudflare embedder heuristic. The exact BPE chunker loads the tokenizer
//! and lives in `lj-ingest`.

/// Median chars per token over the calibration corpus.
pub const CHARS_PER_TOKEN_MEDIAN: f64 = 3.41;
pub const CHARS_PER_TOKEN_STDEV: f64 = 0.124;
pub const CHARS_PER_TOKEN_SAFETY_SIGMAS: f64 = 1.7;
/// 3.20 (MEDIAN - 1.7 sigma, rounded): the conservative ratio of the char-mode budget.
pub const CHARS_PER_TOKEN_SAFE: f64 = 3.20;
