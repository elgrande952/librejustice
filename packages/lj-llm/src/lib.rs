//! `lj-llm` — backends d'inférence modèle : embeddings + client Mistral (chat/OCR).
//!
//! Embeddings : Cloudflare Workers AI, OpenAI-HTTP, cache
//! in-process (`moka`), quantisation L2 + sérialisation vector JSON. Client
//! Mistral (`mistral`) : chat completions + OCR document, rotation de clés et
//! registre de quota (`ratelimit`), partagé entre l'ingest (résumés, OCR CNDA),
//! l'API (rerank listwise) et les bancs.

pub mod backend;
pub mod cache;
pub mod cloudflare;
pub mod error;
pub mod mistral;
pub mod openai_http;
pub mod quantize;
pub mod ratelimit;
