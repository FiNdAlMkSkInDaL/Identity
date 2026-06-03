pub const EMBEDDING_DIM: usize = 384;
pub const EMBEDDING_MODEL_ID: &str = "identity-hash-embedding-v1";
pub const EMBEDDING_LATENCY_TARGET_MS: u128 = 200;
pub const EMBEDDING_RUNTIME_KIND: &str = "prototype-hash";
pub const EMBEDDING_RUNTIME_STATUS: &str = "prototype";
pub const EMBEDDING_ACCELERATION: &str = "cpu-deterministic";
pub const EMBEDDING_QUANTIZATION: &str = "none";
pub const EMBEDDING_ONNX_MODEL_ID: &str = "identity-onnx-embedding-v1";
pub const EMBEDDING_RUNTIME_ENV: &str = "IDENTITY_EMBEDDING_RUNTIME";
pub const EMBEDDING_RUNTIME_ONNX: &str = "onnx";
pub const EMBEDDING_RUNTIME_HASH: &str = "hash";
pub const EMBEDDING_ONNX_MODEL_PATH_ENV: &str = "IDENTITY_EMBEDDING_MODEL_PATH";
pub const EMBEDDING_ONNX_DYLIB_PATH_ENV: &str = "ORT_DYLIB_PATH";
pub const EMBEDDING_TOKENIZER_VOCAB_PATH_ENV: &str = "IDENTITY_TOKENIZER_VOCAB_PATH";
const EMBEDDING_MANIFEST_SUFFIX: &str = ".identity.json";
const EMBEDDING_MANIFEST_MAX_BYTES: u64 = 16 * 1024;
const EMBEDDING_MANIFEST_MODEL_ID_MAX_BYTES: usize = 256;
const EMBEDDING_TOKENIZER_VOCAB_MAX_BYTES: u64 = 4 * 1024 * 1024;
pub const EMBEDDING_TOKENIZER_DEFAULT_MAX_TOKENS: usize = 256;
const EMBEDDING_TOKENIZER_MIN_TOKENS: usize = 4;
const EMBEDDING_TOKENIZER_MAX_TOKENS: usize = 512;

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingProbe {
    pub model_id: &'static str,
    pub dimension: usize,
    pub latency_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingRuntimeInfo {
    pub model_id: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnnxRuntimeHealth {
    pub feature_enabled: bool,
    pub dylib_env_var: &'static str,
    pub dylib_path_configured: bool,
    pub artifact_status: &'static str,
    pub session_status: String,
    pub load_ms: Option<u128>,
    pub input_count: Option<usize>,
    pub output_count: Option<usize>,
    pub first_input: Option<String>,
    pub first_output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenizerHealth {
    pub env_var: &'static str,
    pub configured: bool,
    pub path: Option<String>,
    pub exists: bool,
    pub is_file: bool,
    pub size_bytes: Option<u64>,
    pub token_count: Option<usize>,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenizedInput {
    pub tokens: Vec<String>,
    pub input_ids: Vec<i64>,
    pub attention_mask: Vec<i64>,
    pub token_type_ids: Vec<i64>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OnnxEmbeddingRun {
    pub model_path: String,
    pub vocab_path: String,
    pub token_count: usize,
    pub truncated: bool,
    pub output_floats: usize,
    pub pooled_rows: usize,
    pub embedding: [f32; EMBEDDING_DIM],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveEmbeddingHealth {
    pub env_var: &'static str,
    pub requested_runtime: String,
    pub active_runtime: &'static str,
    pub fallback_reason: Option<String>,
}

impl EmbeddingArtifactHealth {
    pub fn is_ready(&self) -> bool {
        self.status == "ready"
    }
}

#[derive(Clone)]
pub struct EmbeddingEngine {
    runtime: &'static str,
    model_id: String,
}

impl Default for EmbeddingEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbeddingEngine {
    pub fn new() -> Self {
        let health = active_embedding_health();
        let artifact = embedding_artifact_health();
        Self::from_active_health(&health, &artifact)
    }

    pub fn hash() -> Self {
        Self {
            runtime: EMBEDDING_RUNTIME_HASH,
            model_id: EMBEDDING_MODEL_ID.to_string(),
        }
    }

    pub fn from_active_health(
        health: &ActiveEmbeddingHealth,
        artifact: &EmbeddingArtifactHealth,
    ) -> Self {
        if health.active_runtime == EMBEDDING_RUNTIME_ONNX {
            return Self {
                runtime: EMBEDDING_RUNTIME_ONNX,
                model_id: artifact
                    .manifest_model_id
                    .as_ref()
                    .filter(|model_id| !model_id.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| EMBEDDING_ONNX_MODEL_ID.to_string()),
            };
        }

        Self::hash()
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn runtime(&self) -> &'static str {
        self.runtime
    }

    pub fn dimension(&self) -> usize {
        EMBEDDING_DIM
    }

    pub fn blob_len(&self) -> usize {
        self.dimension() * std::mem::size_of::<f32>()
    }

    pub fn runtime_info(&self) -> EmbeddingRuntimeInfo {
        EmbeddingRuntimeInfo {
            model_id: self.model_id().to_string(),
            dimension: self.dimension(),
            runtime_kind: EMBEDDING_RUNTIME_KIND,
            runtime_status: EMBEDDING_RUNTIME_STATUS,
            acceleration: EMBEDDING_ACCELERATION,
            quantization: EMBEDDING_QUANTIZATION,
            onnx_model_path_configured: std::env::var_os(EMBEDDING_ONNX_MODEL_PATH_ENV)
                .map(|path| !path.is_empty())
                .unwrap_or(false)
                || default_model_path().is_some(),
        }
    }

    pub fn artifact_health(&self) -> EmbeddingArtifactHealth {
        embedding_artifact_health()
    }

    pub fn onnx_runtime_health(&self) -> OnnxRuntimeHealth {
        onnx_runtime_health_for_artifact(&self.artifact_health())
    }

    pub fn tokenizer_health(&self) -> TokenizerHealth {
        tokenizer_health()
    }

    pub fn active_embedding_health(&self) -> ActiveEmbeddingHealth {
        active_embedding_health()
    }

    pub fn embed(&self, text: &str) -> [f32; EMBEDDING_DIM] {
        if self.runtime == EMBEDDING_RUNTIME_ONNX {
            if let Ok(run) = try_onnx_embed(text) {
                return run.embedding;
            }
        }

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
    // Env-var path first
    let path = std::env::var_os(EMBEDDING_ONNX_MODEL_PATH_ENV)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from);

    if path.is_some() {
        return embedding_artifact_health_for_path(path);
    }

    // Fall back to default bootstrap directory
    if let Some(default) = default_model_path() {
        if default.exists() {
            return embedding_artifact_health_for_path(Some(default));
        }
    }

    embedding_artifact_health_for_path(None)
}

pub fn onnx_runtime_health() -> OnnxRuntimeHealth {
    let artifact = embedding_artifact_health();
    onnx_runtime_health_for_artifact(&artifact)
}

pub fn tokenizer_health() -> TokenizerHealth {
    // Env-var path first
    let path = std::env::var_os(EMBEDDING_TOKENIZER_VOCAB_PATH_ENV)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from);

    if path.is_some() {
        return tokenizer_health_for_path(path);
    }

    // Fall back to default bootstrap directory
    if let Some(home) = home_dir() {
        let default_vocab = home.join(".identity").join("identity.me").join("models").join("vocab.txt");
        if default_vocab.exists() {
            return tokenizer_health_for_path(Some(default_vocab));
        }
    }

    tokenizer_health_for_path(None)
}

pub fn active_embedding_health() -> ActiveEmbeddingHealth {
    let requested_runtime = requested_embedding_runtime();
    let explicit = is_embedding_runtime_explicitly_set();

    // Default or explicit hash: if requested_runtime is hash, use hash immediately without probing.
    if requested_runtime == EMBEDDING_RUNTIME_HASH {
        return ActiveEmbeddingHealth {
            env_var: EMBEDDING_RUNTIME_ENV,
            requested_runtime,
            active_runtime: EMBEDDING_RUNTIME_HASH,
            fallback_reason: Some(if explicit {
                "explicitly set to hash".to_string()
            } else {
                "defaults to hash".to_string()
            }),
        };
    }

    // Try ONNX: requested_runtime is onnx (or something else), so attempt it
    match try_onnx_probe() {
        Ok(_) => ActiveEmbeddingHealth {
            env_var: EMBEDDING_RUNTIME_ENV,
            requested_runtime,
            active_runtime: EMBEDDING_RUNTIME_ONNX,
            fallback_reason: None,
        },
        Err(error) => ActiveEmbeddingHealth {
            env_var: EMBEDDING_RUNTIME_ENV,
            requested_runtime,
            active_runtime: EMBEDDING_RUNTIME_HASH,
            fallback_reason: Some(error.to_string()),
        },
    }
}

fn is_embedding_runtime_explicitly_set() -> bool {
    std::env::var_os(EMBEDDING_RUNTIME_ENV)
        .map(|val| !val.is_empty())
        .unwrap_or(false)
}

fn requested_embedding_runtime() -> String {
    std::env::var(EMBEDDING_RUNTIME_ENV)
        .map(|value| value.trim().to_ascii_lowercase())
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| EMBEDDING_RUNTIME_HASH.to_string())
}

fn default_model_path() -> Option<std::path::PathBuf> {
    home_dir().map(|h| h.join(".identity").join("identity.me").join("models").join("model.onnx"))
}

/// Attempts an ONNX embedding probe using configured env vars or default model paths.
fn try_onnx_probe() -> Result<OnnxEmbeddingRun, std::io::Error> {
    // 1. Try configured env-var paths
    let model_path = std::env::var_os(EMBEDDING_ONNX_MODEL_PATH_ENV)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from);
    let vocab_path = std::env::var_os(EMBEDDING_TOKENIZER_VOCAB_PATH_ENV)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from);

    match (model_path, vocab_path) {
        (Some(model), Some(vocab)) => {
            return run_onnx_embedding_file(
                &model, &vocab,
                "Identity local embedding runtime health probe.",
                EMBEDDING_TOKENIZER_DEFAULT_MAX_TOKENS,
            );
        }
        (Some(_), None) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{EMBEDDING_ONNX_MODEL_PATH_ENV} is set but {EMBEDDING_TOKENIZER_VOCAB_PATH_ENV} is not configured"),
            ));
        }
        (None, Some(_)) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{EMBEDDING_TOKENIZER_VOCAB_PATH_ENV} is set but {EMBEDDING_ONNX_MODEL_PATH_ENV} is not configured"),
            ));
        }
        (None, None) => {}
    }

    // 2. No env vars: try default bootstrap model directory
    let home = home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound,
            "home directory unavailable; set IDENTITY_EMBEDDING_MODEL_PATH and IDENTITY_TOKENIZER_VOCAB_PATH")
    })?;
    let default_model = home.join(".identity").join("identity.me").join("models").join("model.onnx");
    let default_vocab = home.join(".identity").join("identity.me").join("models").join("vocab.txt");

    if default_model.exists() && default_vocab.exists() {
        return run_onnx_embedding_file(
            &default_model, &default_vocab,
            "Identity local embedding runtime health probe.",
            EMBEDDING_TOKENIZER_DEFAULT_MAX_TOKENS,
        );
    }

    Err(std::io::Error::new(std::io::ErrorKind::NotFound,
        "ONNX embedding model not configured; set IDENTITY_EMBEDDING_MODEL_PATH and IDENTITY_TOKENIZER_VOCAB_PATH, or run embedding-bootstrap"))
}

fn home_dir() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(std::path::PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(std::path::PathBuf::from)
    }
}

/// Resolve ONNX model/vocab paths from env vars or default bootstrap directory,
/// then run embedding. Used at promotion/search time so auto-detected ONNX
/// engines work without env vars on every invocation.
fn try_onnx_embed(text: &str) -> Result<OnnxEmbeddingRun, std::io::Error> {
    // 1. Try configured env-var paths
    let model_path = std::env::var_os(EMBEDDING_ONNX_MODEL_PATH_ENV)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from);
    let vocab_path = std::env::var_os(EMBEDDING_TOKENIZER_VOCAB_PATH_ENV)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from);

    match (model_path, vocab_path) {
        (Some(model), Some(vocab)) => {
            return run_onnx_embedding_file(&model, &vocab, text, EMBEDDING_TOKENIZER_DEFAULT_MAX_TOKENS);
        }
        (Some(_), None) | (None, Some(_)) => { /* fall through to default */ }
        (None, None) => {}
    }

    // 2. Try default bootstrap model directory
    let home = home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "home directory unavailable")
    })?;
    let default_model = home.join(".identity").join("identity.me").join("models").join("model.onnx");
    let default_vocab = home.join(".identity").join("identity.me").join("models").join("vocab.txt");

    if default_model.exists() && default_vocab.exists() {
        return run_onnx_embedding_file(&default_model, &default_vocab, text, EMBEDDING_TOKENIZER_DEFAULT_MAX_TOKENS);
    }

    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "ONNX embedding model not configured"))
}

pub fn tokenizer_health_for_vocab_path(path: &std::path::Path) -> TokenizerHealth {
    tokenizer_health_for_path(Some(path.to_path_buf()))
}

fn tokenizer_health_for_path(path: Option<std::path::PathBuf>) -> TokenizerHealth {
    let Some(path) = path else {
        return TokenizerHealth {
            env_var: EMBEDDING_TOKENIZER_VOCAB_PATH_ENV,
            configured: false,
            path: None,
            exists: false,
            is_file: false,
            size_bytes: None,
            token_count: None,
            status: "not-configured",
        };
    };

    let metadata = std::fs::metadata(&path).ok();
    let exists = metadata.is_some();
    let is_file = metadata.as_ref().map(std::fs::Metadata::is_file).unwrap_or(false);
    let size_bytes = metadata.as_ref().map(std::fs::Metadata::len);
    let mut token_count = None;
    let mut status = if !exists {
        "missing"
    } else if !is_file {
        "not-file"
    } else if size_bytes.unwrap_or(0) == 0 {
        "empty"
    } else if size_bytes.unwrap_or(0) > EMBEDDING_TOKENIZER_VOCAB_MAX_BYTES {
        "too-large"
    } else {
        "ready"
    };

    if status == "ready" {
        match load_wordpiece_vocab(&path) {
            Ok(vocab) => {
                token_count = Some(vocab.len());
                if !vocab.contains_key("[PAD]")
                    || !vocab.contains_key("[UNK]")
                    || !vocab.contains_key("[CLS]")
                    || !vocab.contains_key("[SEP]")
                {
                    status = "missing-special-tokens";
                }
            }
            Err(_) => {
                status = "unreadable";
            }
        }
    }

    TokenizerHealth {
        env_var: EMBEDDING_TOKENIZER_VOCAB_PATH_ENV,
        configured: true,
        path: Some(path.to_string_lossy().into_owned()),
        exists,
        is_file,
        size_bytes,
        token_count,
        status,
    }
}

pub fn tokenize_wordpiece_file(
    vocab_path: &std::path::Path,
    text: &str,
    max_tokens: usize,
) -> Result<TokenizedInput, std::io::Error> {
    validate_token_limit(max_tokens)?;
    let vocab = load_wordpiece_vocab(vocab_path)?;
    tokenize_wordpiece_with_vocab(&vocab, text, max_tokens)
}

pub fn run_onnx_embedding_file(
    model_path: &std::path::Path,
    vocab_path: &std::path::Path,
    text: &str,
    max_tokens: usize,
) -> Result<OnnxEmbeddingRun, std::io::Error> {
    if !cfg!(feature = "onnx-runtime") {
        let _ = (model_path, vocab_path, text, max_tokens);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "identityd was built without --features onnx-runtime",
        ));
    }

    let artifact = embedding_artifact_health_for_model_path(model_path);
    if !artifact.is_ready() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("embedding model artifact is not ready: {}", artifact.status),
        ));
    }

    if std::env::var_os(EMBEDDING_ONNX_DYLIB_PATH_ENV)
        .map(|path| path.is_empty())
        .unwrap_or(true)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{EMBEDDING_ONNX_DYLIB_PATH_ENV} must point at the local ONNX Runtime dynamic library"),
        ));
    }

    let tokenized = tokenize_wordpiece_file(vocab_path, text, max_tokens)?;
    run_onnx_embedding_session(model_path, vocab_path, tokenized)
}

pub fn run_onnx_embedding_from_env(
    text: &str,
    max_tokens: usize,
) -> Result<OnnxEmbeddingRun, std::io::Error> {
    let model_path = std::env::var_os(EMBEDDING_ONNX_MODEL_PATH_ENV)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{EMBEDDING_ONNX_MODEL_PATH_ENV} is not configured"),
        ))?;
    let vocab_path = std::env::var_os(EMBEDDING_TOKENIZER_VOCAB_PATH_ENV)
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{EMBEDDING_TOKENIZER_VOCAB_PATH_ENV} is not configured"),
        ))?;

    run_onnx_embedding_file(&model_path, &vocab_path, text, max_tokens)
}

fn tokenize_wordpiece_with_vocab(
    vocab: &std::collections::HashMap<String, i64>,
    text: &str,
    max_tokens: usize,
) -> Result<TokenizedInput, std::io::Error> {
    let pad_id = required_token_id(vocab, "[PAD]")?;
    let unk_id = required_token_id(vocab, "[UNK]")?;
    let cls_id = required_token_id(vocab, "[CLS]")?;
    let sep_id = required_token_id(vocab, "[SEP]")?;
    let mut tokens = Vec::with_capacity(max_tokens.min(32));
    let mut input_ids = Vec::with_capacity(max_tokens);
    let mut truncated = false;

    tokens.push("[CLS]".to_string());
    input_ids.push(cls_id);

    for token in basic_wordpiece_tokens(text) {
        let pieces = wordpiece_pieces(&token, vocab, unk_id);
        for (piece, piece_id) in pieces {
            if input_ids.len() + 1 >= max_tokens {
                truncated = true;
                break;
            }
            tokens.push(piece);
            input_ids.push(piece_id);
        }
        if truncated {
            break;
        }
    }

    tokens.push("[SEP]".to_string());
    input_ids.push(sep_id);

    let real_tokens = input_ids.len();
    input_ids.resize(max_tokens, pad_id);

    let mut attention_mask = vec![0; max_tokens];
    attention_mask[..real_tokens].fill(1);
    let token_type_ids = vec![0; max_tokens];

    Ok(TokenizedInput {
        tokens,
        input_ids,
        attention_mask,
        token_type_ids,
        truncated,
    })
}

/// Set to `true` after the ONNX runtime has been successfully loaded into the process.
/// The Snapdragon X Elite cpuinfo library has a TLS destructor that triggers an
/// access-violation (0xc0000005) on process exit after an ORT session is created.
/// Callers that loaded ONNX should call `std::process::exit(0)` at the end of the
/// command to bypass C++ TLS cleanup and avoid the crash.
pub static ORT_WAS_LOADED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "onnx-runtime")]
pub fn ensure_ort_initialized() -> Result<(), String> {
    use std::sync::Once;
    static INIT: Once = Once::new();
    let mut err = None;

    INIT.call_once(|| {
        let path = std::env::var_os(EMBEDDING_ONNX_DYLIB_PATH_ENV)
            .filter(|p| !p.is_empty())
            .map(std::path::PathBuf::from);

        if let Some(path) = path {
            match ort::init_from(path) {
                Ok(builder) => {
                    if !builder.commit() {
                        err = Some("Failed to commit ORT environment".to_string());
                    } else {
                        ORT_WAS_LOADED.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                }
                Err(e) => {
                    err = Some(format!("Failed to initialize ORT from path: {e}"));
                }
            }
        } else {
            let builder = ort::init();
            if !builder.commit() {
                err = Some("Failed to commit default ORT environment".to_string());
            } else {
                ORT_WAS_LOADED.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    });

    if let Some(e) = err {
        Err(e)
    } else {
        Ok(())
    }
}

#[cfg(feature = "onnx-runtime")]
fn run_onnx_embedding_session(
    model_path: &std::path::Path,
    vocab_path: &std::path::Path,
    tokenized: TokenizedInput,
) -> Result<OnnxEmbeddingRun, std::io::Error> {
    ensure_ort_initialized().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    use ort::{
        inputs,
        session::{builder::GraphOptimizationLevel, Session},
        value::TensorRef,
    };

    let builder = Session::builder().map_err(io_other)?;
    let builder = builder
        .with_optimization_level(GraphOptimizationLevel::Level1)
        .map_err(io_other)?;
    let builder = builder
        .with_intra_threads(1)
        .map_err(io_other)?;
    let mut builder = builder
        .with_inter_threads(1)
        .map_err(io_other)?;
    let mut session = builder.commit_from_file(model_path).map_err(io_other)?;

    let input_names: Vec<&str> = session.inputs().iter().map(|input| input.name()).collect();
    if !input_names.contains(&"input_ids") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "ONNX embedding model is missing required input_ids input",
        ));
    }
    if !input_names.contains(&"attention_mask") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "ONNX embedding model is missing required attention_mask input",
        ));
    }

    let shape = [1usize, tokenized.input_ids.len()];
    let input_ids = TensorRef::from_array_view((shape, &*tokenized.input_ids)).map_err(io_other)?;
    let attention_mask = TensorRef::from_array_view((shape, &*tokenized.attention_mask)).map_err(io_other)?;
    let token_type_ids = TensorRef::from_array_view((shape, &*tokenized.token_type_ids)).map_err(io_other)?;

    let outputs = if input_names.contains(&"token_type_ids") {
        session.run(inputs! {
            "input_ids" => input_ids,
            "attention_mask" => attention_mask,
            "token_type_ids" => token_type_ids,
        })
    } else {
        session.run(inputs! {
            "input_ids" => input_ids,
            "attention_mask" => attention_mask,
        })
    }
    .map_err(io_other)?;

    if outputs.len() == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "ONNX embedding model returned no outputs",
        ));
    }

    let (_shape, output) = outputs[0].try_extract_tensor::<f32>().map_err(io_other)?;
    let (embedding, pooled_rows) = pool_output_to_embedding(output)?;

    Ok(OnnxEmbeddingRun {
        model_path: model_path.to_string_lossy().into_owned(),
        vocab_path: vocab_path.to_string_lossy().into_owned(),
        token_count: tokenized.tokens.len(),
        truncated: tokenized.truncated,
        output_floats: output.len(),
        pooled_rows,
        embedding,
    })
}

#[cfg(not(feature = "onnx-runtime"))]
fn run_onnx_embedding_session(
    model_path: &std::path::Path,
    vocab_path: &std::path::Path,
    tokenized: TokenizedInput,
) -> Result<OnnxEmbeddingRun, std::io::Error> {
    let _ = (model_path, vocab_path, tokenized);
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "identityd was built without --features onnx-runtime",
    ))
}

pub fn pool_output_to_embedding(
    output: &[f32],
) -> Result<([f32; EMBEDDING_DIM], usize), std::io::Error> {
    if output.len() < EMBEDDING_DIM || !output.len().is_multiple_of(EMBEDDING_DIM) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "ONNX embedding output has {} floats; expected a multiple of {EMBEDDING_DIM}",
                output.len()
            ),
        ));
    }

    let rows = output.len() / EMBEDDING_DIM;
    let mut embedding = [0.0; EMBEDDING_DIM];

    for row in output.chunks_exact(EMBEDDING_DIM) {
        for (index, value) in row.iter().enumerate() {
            embedding[index] += *value;
        }
    }

    if rows > 1 {
        let scale = 1.0 / rows as f32;
        for value in &mut embedding {
            *value *= scale;
        }
    }

    normalize(&mut embedding);
    Ok((embedding, rows))
}

fn validate_token_limit(max_tokens: usize) -> Result<(), std::io::Error> {
    if !(EMBEDDING_TOKENIZER_MIN_TOKENS..=EMBEDDING_TOKENIZER_MAX_TOKENS).contains(&max_tokens) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "token limit must be in 4..=512",
        ));
    }
    Ok(())
}

fn required_token_id(
    vocab: &std::collections::HashMap<String, i64>,
    token: &str,
) -> Result<i64, std::io::Error> {
    vocab.get(token).copied().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("tokenizer vocab is missing required token {token}"),
        )
    })
}

fn load_wordpiece_vocab(
    path: &std::path::Path,
) -> Result<std::collections::HashMap<String, i64>, std::io::Error> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "tokenizer vocab path is not a file",
        ));
    }
    if metadata.len() == 0 || metadata.len() > EMBEDDING_TOKENIZER_VOCAB_MAX_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "tokenizer vocab size is outside supported bounds",
        ));
    }

    let content = std::fs::read_to_string(path)?;
    let mut vocab = std::collections::HashMap::new();
    for (index, line) in content.lines().enumerate() {
        let token = line.trim();
        if token.is_empty() {
            continue;
        }
        vocab.entry(token.to_string()).or_insert(index as i64);
    }

    if vocab.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "tokenizer vocab contains no tokens",
        ));
    }
    Ok(vocab)
}

fn basic_wordpiece_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            current.push(character);
        } else {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            if is_wordpiece_punctuation(character) {
                tokens.push(character.to_string());
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn wordpiece_pieces(
    token: &str,
    vocab: &std::collections::HashMap<String, i64>,
    unk_id: i64,
) -> Vec<(String, i64)> {
    let chars: Vec<char> = token.chars().collect();
    let mut start = 0;
    let mut pieces = Vec::new();

    while start < chars.len() {
        let mut end = chars.len();
        let mut match_piece = None;

        while start < end {
            let fragment: String = chars[start..end].iter().collect();
            let candidate = if start == 0 {
                fragment
            } else {
                format!("##{fragment}")
            };

            if let Some(piece_id) = vocab.get(&candidate) {
                match_piece = Some((candidate, *piece_id));
                break;
            }
            end -= 1;
        }

        let Some(piece) = match_piece else {
            return vec![("[UNK]".to_string(), unk_id)];
        };

        pieces.push(piece);
        start = end;
    }
    pieces
}

fn is_wordpiece_punctuation(character: char) -> bool {
    character.is_ascii_punctuation()
}

pub fn onnx_runtime_health_for_artifact(artifact: &EmbeddingArtifactHealth) -> OnnxRuntimeHealth {
    let dylib_path_configured = std::env::var_os(EMBEDDING_ONNX_DYLIB_PATH_ENV)
        .map(|path| !path.is_empty())
        .unwrap_or(false);

    if !cfg!(feature = "onnx-runtime") {
        return OnnxRuntimeHealth {
            feature_enabled: false,
            dylib_env_var: EMBEDDING_ONNX_DYLIB_PATH_ENV,
            dylib_path_configured,
            artifact_status: artifact.status,
            session_status: "feature-disabled".to_string(),
            load_ms: None,
            input_count: None,
            output_count: None,
            first_input: None,
            first_output: None,
        };
    }

    if !artifact.is_ready() {
        return OnnxRuntimeHealth {
            feature_enabled: true,
            dylib_env_var: EMBEDDING_ONNX_DYLIB_PATH_ENV,
            dylib_path_configured,
            artifact_status: artifact.status,
            session_status: "artifact-not-ready".to_string(),
            load_ms: None,
            input_count: None,
            output_count: None,
            first_input: None,
            first_output: None,
        };
    }

    if !dylib_path_configured {
        return OnnxRuntimeHealth {
            feature_enabled: true,
            dylib_env_var: EMBEDDING_ONNX_DYLIB_PATH_ENV,
            dylib_path_configured,
            artifact_status: artifact.status,
            session_status: "dylib-not-configured".to_string(),
            load_ms: None,
            input_count: None,
            output_count: None,
            first_input: None,
            first_output: None,
        };
    }

    let Some(path) = artifact.path.as_deref() else {
        return OnnxRuntimeHealth {
            feature_enabled: true,
            dylib_env_var: EMBEDDING_ONNX_DYLIB_PATH_ENV,
            dylib_path_configured,
            artifact_status: artifact.status,
            session_status: "artifact-path-missing".to_string(),
            load_ms: None,
            input_count: None,
            output_count: None,
            first_input: None,
            first_output: None,
        };
    };

    load_onnx_session(path, dylib_path_configured, artifact.status)
}

#[cfg(feature = "onnx-runtime")]
fn load_onnx_session(
    path: &str,
    dylib_path_configured: bool,
    artifact_status: &'static str,
) -> OnnxRuntimeHealth {
    use ort::session::{builder::GraphOptimizationLevel, Session};

    let started = std::time::Instant::now();
    if let Err(e) = ensure_ort_initialized() {
        return OnnxRuntimeHealth {
            feature_enabled: true,
            dylib_env_var: EMBEDDING_ONNX_DYLIB_PATH_ENV,
            dylib_path_configured,
            artifact_status,
            session_status: format!("init-failed: {e}"),
            load_ms: None,
            input_count: None,
            output_count: None,
            first_input: None,
            first_output: None,
        };
    }

    let session = (|| -> Result<Session, String> {
        let builder = Session::builder().map_err(|error| error.to_string())?;
        let builder = builder
            .with_optimization_level(GraphOptimizationLevel::Level1)
            .map_err(|error| error.to_string())?;
        let builder = builder
            .with_intra_threads(1)
            .map_err(|error| error.to_string())?;
        let mut builder = builder
            .with_inter_threads(1)
            .map_err(|error| error.to_string())?;
        builder.commit_from_file(path).map_err(|error| error.to_string())
    })();

    match session {
        Ok(session) => {
            let health = OnnxRuntimeHealth {
                feature_enabled: true,
                dylib_env_var: EMBEDDING_ONNX_DYLIB_PATH_ENV,
                dylib_path_configured,
                artifact_status,
                session_status: "ready".to_string(),
                load_ms: Some(started.elapsed().as_millis()),
                input_count: Some(session.inputs().len()),
                output_count: Some(session.outputs().len()),
                first_input: session.inputs().first().map(|input| input.name().to_string()),
                first_output: session.outputs().first().map(|output| output.name().to_string()),
            };
            // Intentionally leak the session to prevent C++ destructor from running.
            // On Snapdragon X Elite, the cpuinfo thread-local storage destructor inside
            // onnxruntime triggers an access-violation (0xc0000005) during Session::drop().
            // The caller will call std::process::exit(0) after printing output, so no memory
            // is actually stranded in practice.
            std::mem::forget(session);
            health
        }
        Err(error) => OnnxRuntimeHealth {
            feature_enabled: true,
            dylib_env_var: EMBEDDING_ONNX_DYLIB_PATH_ENV,
            dylib_path_configured,
            artifact_status,
            session_status: bounded_status("session-load-failed", &error),
            load_ms: Some(started.elapsed().as_millis()),
            input_count: None,
            output_count: None,
            first_input: None,
            first_output: None,
        },
    }
}

#[cfg(not(feature = "onnx-runtime"))]
fn load_onnx_session(
    _path: &str,
    dylib_path_configured: bool,
    artifact_status: &'static str,
) -> OnnxRuntimeHealth {
    OnnxRuntimeHealth {
        feature_enabled: false,
        dylib_env_var: EMBEDDING_ONNX_DYLIB_PATH_ENV,
        dylib_path_configured,
        artifact_status,
        session_status: "feature-disabled".to_string(),
        load_ms: None,
        input_count: None,
        output_count: None,
        first_input: None,
        first_output: None,
    }
}

pub fn write_embedding_manifest(
    model_path: &std::path::Path,
    model_id: &str,
    overwrite: bool,
) -> Result<std::path::PathBuf, std::io::Error> {
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "embedding model id must not be empty"));
    }
    if model_id.len() > EMBEDDING_MANIFEST_MODEL_ID_MAX_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "embedding model id is too long"));
    }

    let metadata = std::fs::metadata(model_path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "embedding model path is not a file"));
    }
    let has_onnx_extension = model_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("onnx"))
        .unwrap_or(false);
    if !has_onnx_extension {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput,
            "embedding model path must have .onnx extension"));
    }
    if metadata.len() == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "embedding model file is empty"));
    }

    let manifest_path = embedding_manifest_path(model_path);
    if manifest_path.exists() && !overwrite {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists,
            "embedding manifest already exists; pass --force to overwrite it"));
    }

    let manifest = format!(
        "{{\n  \"model_id\": \"{}\",\n  \"embedding_dim\": {}\n}}\n",
        json_escape(model_id),
        EMBEDDING_DIM
    );
    std::fs::write(&manifest_path, manifest)?;
    Ok(manifest_path)
}

pub fn embedding_artifact_health_for_model_path(model_path: &std::path::Path) -> EmbeddingArtifactHealth {
    embedding_artifact_health_for_path(Some(model_path.to_path_buf()))
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
    let is_file = metadata.as_ref().map(std::fs::Metadata::is_file).unwrap_or(false);
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

pub fn embedding_manifest_path(model_path: &std::path::Path) -> std::path::PathBuf {
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
    let mut characters = value.strip_prefix('"')?.chars();
    let mut output = String::new();
    while let Some(character) = characters.next() {
        match character {
            '"' => return Some(output),
            '\\' => match characters.next()? {
                '"' => output.push('"'),
                '\\' => output.push('\\'),
                'n' => output.push('\n'),
                'r' => output.push('\r'),
                't' => output.push('\t'),
                escaped => output.push(escaped),
            },
            value => output.push(value),
        }
    }
    None
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

fn json_escape(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            value => escaped.push(value),
        }
    }
    escaped
}

#[cfg(feature = "onnx-runtime")]
fn bounded_status(prefix: &str, detail: &str) -> String {
    let mut status = String::with_capacity(prefix.len() + 1 + detail.len().min(180));
    status.push_str(prefix);
    status.push(':');
    for character in detail.chars().take(180) {
        match character {
            '\r' | '\n' | '\t' => status.push(' '),
            value => status.push(value),
        }
    }
    status
}

#[cfg(feature = "onnx-runtime")]
fn io_other(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(error.to_string())
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
        from_le_bytes, onnx_runtime_health_for_artifact, probe_embedding_latency, to_le_bytes,
        tokenize_wordpiece_file, tokenizer_health_for_vocab_path, write_embedding_manifest,
        EmbeddingEngine, EMBEDDING_ACCELERATION, EMBEDDING_DIM, EMBEDDING_LATENCY_TARGET_MS,
        EMBEDDING_MODEL_ID, EMBEDDING_QUANTIZATION, EMBEDDING_RUNTIME_KIND,
        EMBEDDING_RUNTIME_STATUS,
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

    #[cfg(not(feature = "onnx-runtime"))]
    #[test]
    fn onnx_runtime_health_is_feature_gated_by_default() {
        let artifact = embedding_artifact_health_for_path(None);
        let health = onnx_runtime_health_for_artifact(&artifact);
        assert!(!health.feature_enabled);
        assert_eq!(health.artifact_status, "not-configured");
        assert_eq!(health.session_status, "feature-disabled");
        assert_eq!(health.load_ms, None);
        assert_eq!(health.input_count, None);
        assert_eq!(health.output_count, None);
    }

    #[cfg(feature = "onnx-runtime")]
    #[test]
    fn onnx_runtime_health_reports_feature_enabled_without_artifact() {
        let artifact = embedding_artifact_health_for_path(None);
        let health = onnx_runtime_health_for_artifact(&artifact);
        assert!(health.feature_enabled);
        assert_eq!(health.artifact_status, "not-configured");
        assert_eq!(health.session_status, "artifact-not-ready");
        assert_eq!(health.load_ms, None);
        assert_eq!(health.input_count, None);
        assert_eq!(health.output_count, None);
    }

    #[test]
    fn tokenizer_health_validates_wordpiece_vocab_shape() {
        let root = temp_test_dir("identity-tokenizer-health-test");
        fs::create_dir_all(&root).unwrap();
        let vocab = root.join("vocab.txt");
        fs::write(&vocab, "[PAD]\n[UNK]\n[CLS]\n[SEP]\nhello\nworld\n##s\n!\n").unwrap();
        let health = tokenizer_health_for_vocab_path(&vocab);
        assert_eq!(health.status, "ready");
        assert_eq!(health.token_count, Some(8));
        assert!(health.configured);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tokenizes_text_into_padded_wordpiece_tensors() {
        let root = temp_test_dir("identity-tokenizer-tokenize-test");
        fs::create_dir_all(&root).unwrap();
        let vocab = root.join("vocab.txt");
        fs::write(&vocab, "[PAD]\n[UNK]\n[CLS]\n[SEP]\nhello\nworld\n##s\n!\nprivate\n").unwrap();
        let tokenized = tokenize_wordpiece_file(&vocab, "Hello worlds! private", 10).unwrap();
        assert_eq!(tokenized.tokens, vec!["[CLS]", "hello", "world", "##s", "!", "private", "[SEP]"]);
        assert_eq!(tokenized.input_ids, vec![2, 4, 5, 6, 7, 8, 3, 0, 0, 0]);
        assert_eq!(tokenized.attention_mask, vec![1, 1, 1, 1, 1, 1, 1, 0, 0, 0]);
        assert_eq!(tokenized.token_type_ids, vec![0; 10]);
        assert!(!tokenized.truncated);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tokenization_uses_unknown_and_reports_truncation() {
        let root = temp_test_dir("identity-tokenizer-unknown-test");
        fs::create_dir_all(&root).unwrap();
        let vocab = root.join("vocab.txt");
        fs::write(&vocab, "[PAD]\n[UNK]\n[CLS]\n[SEP]\nhello\n").unwrap();
        let tokenized = tokenize_wordpiece_file(&vocab, "hello impossible hello", 4).unwrap();
        assert_eq!(tokenized.tokens, vec!["[CLS]", "hello", "[UNK]", "[SEP]"]);
        assert_eq!(tokenized.input_ids, vec![2, 4, 1, 3]);
        assert_eq!(tokenized.attention_mask, vec![1, 1, 1, 1]);
        assert!(tokenized.truncated);
        assert!(tokenize_wordpiece_file(&vocab, "hello", 3).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pools_onnx_output_rows_into_normalized_embedding() {
        let mut output = vec![0.0; EMBEDDING_DIM * 2];
        output[0] = 1.0;
        output[EMBEDDING_DIM] = 1.0;
        let (embedding, rows) = super::pool_output_to_embedding(&output).unwrap();
        assert_eq!(rows, 2);
        assert_eq!(embedding[0], 1.0);
        assert!(embedding[1..].iter().all(|value| *value == 0.0));
        assert!(super::pool_output_to_embedding(&output[..EMBEDDING_DIM - 1]).is_err());
    }

    #[test]
    fn embedding_artifact_health_validates_configured_model_path() {
        let root = std::env::temp_dir().join(format!(
            "identity-embedding-artifact-test-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let valid = root.join("model.onnx");
        let wrong_extension = root.join("model.bin");
        fs::write(&valid, [1_u8, 2, 3, 4]).unwrap();
        fs::write(&embedding_manifest_path(&valid),
            format!(r#"{{"model_id":"test-minilm","embedding_dim":{}}}"#, EMBEDDING_DIM)).unwrap();
        fs::write(&wrong_extension, [1_u8]).unwrap();
        let empty = root.join("empty.onnx");
        fs::write(&empty, []).unwrap();
        let no_manifest = root.join("no-manifest.onnx");
        fs::write(&no_manifest, [1_u8]).unwrap();
        let wrong_dimension = root.join("wrong-dim.onnx");
        fs::write(&wrong_dimension, [1_u8]).unwrap();
        fs::write(&embedding_manifest_path(&wrong_dimension),
            r#"{"model_id":"wrong-dim","embedding_dim":768}"#).unwrap();

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

    #[test]
    fn writes_embedding_manifest_for_existing_onnx_artifact() {
        let root = temp_test_dir("identity-embedding-manifest-write-test");
        fs::create_dir_all(&root).unwrap();
        let model = root.join("model.onnx");
        fs::write(&model, [1_u8, 2, 3, 4]).unwrap();
        let manifest_path = write_embedding_manifest(&model, "mini\"lm", false).unwrap();
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        let health = embedding_artifact_health_for_path(Some(model));
        assert!(manifest.contains(r#""model_id": "mini\"lm""#));
        assert!(manifest.contains(r#""embedding_dim": 384"#));
        assert_eq!(health.status, "ready");
        assert_eq!(health.manifest_model_id.as_deref(), Some("mini\"lm"));
        assert_eq!(health.manifest_embedding_dim, Some(EMBEDDING_DIM));
        assert!(write_embedding_manifest(&root.join("model.onnx"), "other", false).is_err());
        assert!(write_embedding_manifest(&root.join("model.onnx"), "other", true).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_manifest_for_invalid_model_artifacts() {
        let root = temp_test_dir("identity-embedding-manifest-refuse-test");
        fs::create_dir_all(&root).unwrap();
        let empty = root.join("empty.onnx");
        let wrong = root.join("model.bin");
        fs::write(&empty, []).unwrap();
        fs::write(&wrong, [1_u8]).unwrap();
        assert!(write_embedding_manifest(&root, "dir", false).is_err());
        assert!(write_embedding_manifest(&empty, "empty", false).is_err());
        assert!(write_embedding_manifest(&wrong, "wrong", false).is_err());
        assert!(write_embedding_manifest(&wrong, "", false).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    fn temp_test_dir(prefix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ))
    }
}
