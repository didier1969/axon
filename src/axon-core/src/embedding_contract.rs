use fastembed::EmbeddingModel;

pub const MODEL_NAME: &str = "BAAI/bge-large-en-v1.5";
pub const MODEL_VERSION: &str = "1";
pub const DIMENSION: usize = 1024;
pub const NATIVE_DIMENSION: usize = DIMENSION;
pub const MAX_LENGTH: usize = 512;
pub const STORAGE_TYPE: &str = "float16";

pub const SYMBOL_MODEL_ID: &str = "sym-bge-large-en-v1.5-1024";
pub const CHUNK_MODEL_ID: &str = "chunk-bge-large-en-v1.5-1024";
pub const GRAPH_MODEL_ID: &str = "graph-bge-large-en-v1.5-1024";

pub fn fastembed_model() -> EmbeddingModel {
    EmbeddingModel::BGELargeENV15
}
