pub const EMBEDDING_DIM: usize = 384;
pub const EMBEDDING_MODEL_ID: &str = "identity-hash-embedding-v1";
pub const EMBEDDING_LATENCY_TARGET_MS: u128 = 200;
pub const EMBEDDING_RUNTIME_KIND: &str = "prototype-hash";
pub const EMBEDDING_RUNTIME_STATUS: &str = "prototype";
pub const EMBEDDING_ACCELERATION: &str = "cpu-deterministic";
pub const EMBEDDING_QUANTIZATION: &str = "none";
pub const EMBEDDING_ONNX_MODEL_PATH_ENV: &str = "IDENTITY_EMBEDDING_MODEL_PATH";
const EMBEDDING_MANIFEST_SUFFIX: &str = ".identity.json";
const EMBEDDING_MANIFEST_MAX_BYTES: u64 = 16 * 1024;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingArtifactHealth {
    pub env_var: &'static str,
    pub configured: bool,
    pub path: Option<String>,
    pub exists: bool,
    pub is_file: bool,
    pub has_onnx_extension: bool,
    pub size_bytes: Option<u64>,
    pub manifest_path: Option<String>,
    pub manifest_exists: bool,
    pub manifest_size_bytes: Option<u64>,
    pub manifest_model_id: Option<String>,
    pub manifest_embedding_dim: Option<usize>,
    pub status: &'static str,
}

impl EmbeddingArtifactHealth {
    pub fn is_ready(&self) -> bool {
        self.status == "ready"
    }
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
            onnx_model_path_configured: std::env::var_os(EMBEDDING_ONNX_MODEL_PATH_ENV)
                .map(|path| !path.is_empty())
                .unwrap_or(false),
        }
    }

    pub fn artifact_health(&self) -> EmbeddingArtifactHealth {
        embedding_artifact_health()
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

pub fn embedding_artifact_health() -> EmbeddingArtifactHealth {
    let path = std::env::var_os(EMBEDDING_ONNX_MODEL_PATH_ENV)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from);
    embedding_artifact_health_for_path(path)
}

fn embedding_artifact_health_for_path(path: Option<std::path::PathBuf>) -> EmbeddingArtifactHealth {
    let Some(path) = path else {
        return EmbeddingArtifactHealth {
            env_var: EMBEDDING_ONNX_MODEL_PATH_ENV,
            configured: false,
            path: None,
            exists: false,
            is_file: false,
            has_onnx_extension: false,
            size_bytes: None,
            manifest_path: None,
            manifest_exists: false,
            manifest_size_bytes: None,
            manifest_model_id: None,
            manifest_embedding_dim: None,
            status: "not-configured",
        };
    };

    let metadata = std::fs::metadata(&path).ok();
    let exists = metadata.is_some();
    let is_file = metadata
        .as_ref()
        .map(std::fs::Metadata::is_file)
        .unwrap_or(false);
    let has_onnx_extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("onnx"))
        .unwrap_or(false);
    let size_bytes = metadata.as_ref().map(std::fs::Metadata::len);
    let mut manifest_path = None;
    let mut manifest_exists = false;
    let mut manifest_size_bytes = None;
    let mut manifest_model_id = None;
    let mut manifest_embedding_dim = None;

    let mut status = if !exists {
        "missing"
    } else if !is_file {
        "not-file"
    } else if !has_onnx_extension {
        "wrong-extension"
    } else if size_bytes.unwrap_or(0) == 0 {
        "empty"
    } else {
        "ready"
    };

    if status == "ready" {
        let path = embedding_manifest_path(&path);
        manifest_path = Some(path.to_string_lossy().into_owned());

        match read_embedding_manifest(&path) {
            ManifestRead::Missing => {
                status = "manifest-missing";
            }
            ManifestRead::TooLarge(size) => {
                manifest_exists = true;
                manifest_size_bytes = Some(size);
                status = "manifest-too-large";
            }
            ManifestRead::Unreadable => {
                manifest_exists = true;
                status = "manifest-unreadable";
            }
            ManifestRead::Invalid(size) => {
                manifest_exists = true;
                manifest_size_bytes = Some(size);
                status = "manifest-invalid";
            }
            ManifestRead::Ready(manifest) => {
                manifest_exists = true;
                manifest_size_bytes = Some(manifest.size_bytes);
                manifest_model_id = manifest.model_id;
                manifest_embedding_dim = manifest.embedding_dim;

                if manifest_embedding_dim != Some(EMBEDDING_DIM) {
                    status = "dimension-mismatch";
                }
            }
        }
    }

    EmbeddingArtifactHealth {
        env_var: EMBEDDING_ONNX_MODEL_PATH_ENV,
        configured: true,
        path: Some(path.to_string_lossy().into_owned()),
        exists,
        is_file,
        has_onnx_extension,
        size_bytes,
        manifest_path,
        manifest_exists,
        manifest_size_bytes,
        manifest_model_id,
        manifest_embedding_dim,
        status,
    }
}

struct EmbeddingManifest {
    size_bytes: u64,
    model_id: Option<String>,
    embedding_dim: Option<usize>,
}

enum ManifestRead {
    Missing,
    TooLarge(u64),
    Unreadable,
    Invalid(u64),
    Ready(EmbeddingManifest),
}

fn embedding_manifest_path(model_path: &std::path::Path) -> std::path::PathBuf {
    let mut path = model_path.as_os_str().to_os_string();
    path.push(EMBEDDING_MANIFEST_SUFFIX);
    std::path::PathBuf::from(path)
}

fn read_embedding_manifest(path: &std::path::Path) -> ManifestRead {
    let Ok(metadata) = std::fs::metadata(path) else {
        return ManifestRead::Missing;
    };

    if !metadata.is_file() {
        return ManifestRead::Invalid(metadata.len());
    }

    let size_bytes = metadata.len();
    if size_bytes > EMBEDDING_MANIFEST_MAX_BYTES {
        return ManifestRead::TooLarge(size_bytes);
    }

    let Ok(content) = std::fs::read_to_string(path) else {
        return ManifestRead::Unreadable;
    };

    let model_id = json_string_field(&content, "model_id");
    let embedding_dim = json_usize_field(&content, "embedding_dim");

    if embedding_dim.is_none() {
        return ManifestRead::Invalid(size_bytes);
    }

    ManifestRead::Ready(EmbeddingManifest {
        size_bytes,
        model_id,
        embedding_dim,
    })
}

fn json_string_field(content: &str, key: &str) -> Option<String> {
    let value = json_field_value(content, key)?.trim_start();
    let value = value.strip_prefix('"')?;
    let end = value.find('"')?;
    Some(value[..end].to_string())
}

fn json_usize_field(content: &str, key: &str) -> Option<usize> {
    let value = json_field_value(content, key)?.trim_start();
    let end = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());

    if end == 0 {
        return None;
    }

    value[..end].parse::<usize>().ok()
}

fn json_field_value<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\"{key}\"");
    let value = content.split_once(&marker)?.1;
    value.split_once(':').map(|(_, value)| value)
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
        cosine_similarity, embed_text, embedding_artifact_health_for_path, embedding_manifest_path,
        from_le_bytes, probe_embedding_latency, to_le_bytes, EmbeddingEngine,
        EMBEDDING_ACCELERATION, EMBEDDING_DIM, EMBEDDING_LATENCY_TARGET_MS, EMBEDDING_MODEL_ID,
        EMBEDDING_QUANTIZATION, EMBEDDING_RUNTIME_KIND, EMBEDDING_RUNTIME_STATUS,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[test]
    fn embedding_artifact_health_reports_not_configured() {
        let health = embedding_artifact_health_for_path(None);

        assert_eq!(health.status, "not-configured");
        assert!(!health.is_ready());
        assert!(!health.configured);
    }

    #[test]
    fn embedding_artifact_health_validates_configured_model_path() {
        let root = std::env::temp_dir().join(format!(
            "identity-embedding-artifact-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let valid = root.join("model.onnx");
        let wrong_extension = root.join("model.bin");
        fs::write(&valid, [1_u8, 2, 3, 4]).unwrap();
        fs::write(
            embedding_manifest_path(&valid),
            format!(
                r#"{{"model_id":"test-minilm","embedding_dim":{}}}"#,
                EMBEDDING_DIM
            ),
        )
        .unwrap();
        fs::write(&wrong_extension, [1_u8]).unwrap();
        let empty = root.join("empty.onnx");
        fs::write(&empty, []).unwrap();
        let no_manifest = root.join("no-manifest.onnx");
        fs::write(&no_manifest, [1_u8]).unwrap();
        let wrong_dimension = root.join("wrong-dim.onnx");
        fs::write(&wrong_dimension, [1_u8]).unwrap();
        fs::write(
            embedding_manifest_path(&wrong_dimension),
            r#"{"model_id":"wrong-dim","embedding_dim":768}"#,
        )
        .unwrap();

        let ready = embedding_artifact_health_for_path(Some(valid.clone()));
        let wrong = embedding_artifact_health_for_path(Some(wrong_extension));
        let missing = embedding_artifact_health_for_path(Some(root.join("missing.onnx")));
        let empty = embedding_artifact_health_for_path(Some(empty));
        let no_manifest = embedding_artifact_health_for_path(Some(no_manifest));
        let wrong_dimension = embedding_artifact_health_for_path(Some(wrong_dimension));

        assert_eq!(ready.status, "ready");
        assert!(ready.is_ready());
        assert_eq!(ready.size_bytes, Some(4));
        assert!(ready.manifest_exists);
        assert_eq!(ready.manifest_model_id.as_deref(), Some("test-minilm"));
        assert_eq!(ready.manifest_embedding_dim, Some(EMBEDDING_DIM));
        assert!(ready.path.unwrap().contains("model.onnx"));
        assert_eq!(wrong.status, "wrong-extension");
        assert!(!wrong.is_ready());
        assert_eq!(missing.status, "missing");
        assert!(!missing.is_ready());
        assert_eq!(empty.status, "empty");
        assert!(!empty.is_ready());
        assert_eq!(no_manifest.status, "manifest-missing");
        assert!(!no_manifest.is_ready());
        assert_eq!(wrong_dimension.status, "dimension-mismatch");
        assert_eq!(wrong_dimension.manifest_embedding_dim, Some(768));
        assert!(!wrong_dimension.is_ready());

        fs::remove_dir_all(root).unwrap();
    }
}
