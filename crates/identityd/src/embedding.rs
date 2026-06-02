pub const EMBEDDING_DIM: usize = 384;
pub const EMBEDDING_MODEL_ID: &str = "identity-hash-embedding-v1";
pub const EMBEDDING_LATENCY_TARGET_MS: u128 = 200;
pub const EMBEDDING_RUNTIME_KIND: &str = "prototype-hash";
pub const EMBEDDING_RUNTIME_STATUS: &str = "prototype";
pub const EMBEDDING_ACCELERATION: &str = "cpu-deterministic";
pub const EMBEDDING_QUANTIZATION: &str = "none";
pub const EMBEDDING_ONNX_MODEL_PATH_ENV: &str = "IDENTITY_EMBEDDING_MODEL_PATH";

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingProbe {
    pub model_id: &'static str,
    pub dimension: usize,
    pub latency_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingRuntimeInfo {
    pub model_id: &'static str,
    pub dimension: usize,
    pub runtime_kind: &'static str,
    pub runtime_status: &'static str,
    pub acceleration: &'static str,
    pub quantization: &'static str,
    pub onnx_model_path_configured: bool,
}

#[derive(Clone, Copy)]
pub struct EmbeddingEngine;

impl Default for EmbeddingEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbeddingEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn model_id(&self) -> &'static str {
        EMBEDDING_MODEL_ID
    }

    pub fn dimension(&self) -> usize {
        EMBEDDING_DIM
    }

    pub fn blob_len(&self) -> usize {
        self.dimension() * std::mem::size_of::<f32>()
    }

    pub fn runtime_info(&self) -> EmbeddingRuntimeInfo {
        EmbeddingRuntimeInfo {
            model_id: self.model_id(),
            dimension: self.dimension(),
            runtime_kind: EMBEDDING_RUNTIME_KIND,
            runtime_status: EMBEDDING_RUNTIME_STATUS,
            acceleration: EMBEDDING_ACCELERATION,
            quantization: EMBEDDING_QUANTIZATION,
            onnx_model_path_configured: std::env::var_os(EMBEDDING_ONNX_MODEL_PATH_ENV).is_some(),
        }
    }

    pub fn embed(&self, text: &str) -> [f32; EMBEDDING_DIM] {
        embed_text(text)
    }

    pub fn encode_bytes(&self, text: &str) -> Vec<u8> {
        to_le_bytes(&self.embed(text))
    }

    pub fn resolve_bytes(
        &self,
        primary: Option<&[u8]>,
        secondary: Option<&[u8]>,
        text: &str,
    ) -> [f32; EMBEDDING_DIM] {
        primary
            .and_then(from_le_bytes)
            .or_else(|| secondary.and_then(from_le_bytes))
            .unwrap_or_else(|| self.embed(text))
    }

    pub fn similarity(&self, left: &[f32; EMBEDDING_DIM], right: &[f32; EMBEDDING_DIM]) -> f32 {
        cosine_similarity(left, right)
    }
}

pub fn embed_text(input: &str) -> [f32; EMBEDDING_DIM] {
    let mut vector = [0.0; EMBEDDING_DIM];

    for token in tokens(input) {
        let hash = stable_hash(token.as_bytes());
        let index = (hash as usize) % EMBEDDING_DIM;
        let sign = if hash & 1 == 0 { 1.0 } else { -1.0 };
        vector[index] += sign;
    }

    normalize(&mut vector);
    vector
}

pub fn probe_embedding_latency(input: &str) -> EmbeddingProbe {
    let started = std::time::Instant::now();
    let _embedding = embed_text(input);

    EmbeddingProbe {
        model_id: EMBEDDING_MODEL_ID,
        dimension: EMBEDDING_DIM,
        latency_ms: started.elapsed().as_millis(),
    }
}

pub fn to_le_bytes(vector: &[f32; EMBEDDING_DIM]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(EMBEDDING_DIM * std::mem::size_of::<f32>());

    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    bytes
}

pub fn from_le_bytes(bytes: &[u8]) -> Option<[f32; EMBEDDING_DIM]> {
    if bytes.len() != EMBEDDING_DIM * std::mem::size_of::<f32>() {
        return None;
    }

    let mut vector = [0.0; EMBEDDING_DIM];

    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        vector[index] = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }

    Some(vector)
}

pub fn cosine_similarity(left: &[f32; EMBEDDING_DIM], right: &[f32; EMBEDDING_DIM]) -> f32 {
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| left * right)
        .sum::<f32>()
        .max(0.0)
}

fn tokens(input: &str) -> impl Iterator<Item = &str> {
    input
        .split(|character: char| !character.is_ascii_alphanumeric())
        .map(str::trim)
        .filter(|token| token.len() >= 3)
}

fn normalize(vector: &mut [f32; EMBEDDING_DIM]) {
    let magnitude = vector.iter().map(|value| value * value).sum::<f32>().sqrt();

    if magnitude == 0.0 {
        return;
    }

    for value in vector {
        *value /= magnitude;
    }
}

#[inline]
fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;

    for byte in bytes {
        hash ^= u64::from(byte.to_ascii_lowercase());
        hash = hash.wrapping_mul(0x100000001b3);
    }

    hash
}

#[cfg(test)]
mod tests {
    use super::{
        cosine_similarity, embed_text, from_le_bytes, probe_embedding_latency, to_le_bytes,
        EmbeddingEngine, EMBEDDING_ACCELERATION, EMBEDDING_DIM, EMBEDDING_LATENCY_TARGET_MS,
        EMBEDDING_MODEL_ID, EMBEDDING_QUANTIZATION, EMBEDDING_RUNTIME_KIND,
        EMBEDDING_RUNTIME_STATUS,
    };

    #[test]
    fn embedding_round_trips_as_fixed_width_little_endian_blob() {
        let embedding = embed_text("local private memory");
        let bytes = to_le_bytes(&embedding);
        let decoded = from_le_bytes(&bytes).unwrap();

        assert_eq!(bytes.len(), EMBEDDING_DIM * 4);
        assert_eq!(embedding, decoded);
    }

    #[test]
    fn related_text_scores_higher_than_unrelated_text() {
        let query = embed_text("private memory");
        let related = embed_text("local private memory belongs on device");
        let unrelated = embed_text("weather forecast tomorrow");

        assert!(cosine_similarity(&query, &related) > cosine_similarity(&query, &unrelated));
    }

    #[test]
    fn embedding_probe_reports_model_dimension_and_latency() {
        let probe = probe_embedding_latency("Identity maps local context into private memory.");

        assert_eq!(probe.model_id, EMBEDDING_MODEL_ID);
        assert_eq!(probe.dimension, EMBEDDING_DIM);
        assert!(probe.latency_ms < EMBEDDING_LATENCY_TARGET_MS);
    }

    #[test]
    fn embedding_engine_reports_runtime_boundary_metadata() {
        let engine = EmbeddingEngine::new();
        let info = engine.runtime_info();

        assert_eq!(info.model_id, EMBEDDING_MODEL_ID);
        assert_eq!(info.dimension, EMBEDDING_DIM);
        assert_eq!(info.runtime_kind, EMBEDDING_RUNTIME_KIND);
        assert_eq!(info.runtime_status, EMBEDDING_RUNTIME_STATUS);
        assert_eq!(info.acceleration, EMBEDDING_ACCELERATION);
        assert_eq!(info.quantization, EMBEDDING_QUANTIZATION);
        assert_eq!(engine.blob_len(), EMBEDDING_DIM * 4);
    }
}
