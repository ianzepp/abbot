// Vector-based semantic memory for transcript search.
//
// Indexes conversation transcripts into sqlite-vec for fuzzy recall queries
// like "why did we decide X?" or "what did we discuss about Y?".

mod schema;
mod indexer;
mod search;
mod ollama;

pub use schema::ensure_schema;
pub use indexer::{Indexer, ChunkMeta};
pub use search::{Search, SearchResult};
pub use ollama::Ollama;
