// Vector-based semantic memory for transcript search.
//
// Indexes conversation transcripts into sqlite-vec for fuzzy recall queries
// like "why did we decide X?" or "what did we discuss about Y?".

mod indexer;
mod ollama;
mod schema;
mod search;

pub use indexer::{ChunkMeta, Indexer};
pub use ollama::Ollama;
pub use schema::ensure_schema;
pub use search::{Search, SearchResult};
