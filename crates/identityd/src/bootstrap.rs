// ONNX artifact bootstrap -- downloads all-MiniLM-L6-v2 model + vocab.
//
// Uses the system `curl.exe` (shipped with Windows 10+) for zero-dependency HTTPS
// downloads. Existing files are not re-downloaded.

use crate::embedding::{
    embedding_artifact_health_for_model_path, embedding_manifest_path,
    tokenizer_health_for_vocab_path, write_embedding_manifest, EMBEDDING_ONNX_DYLIB_PATH_ENV,
    EMBEDDING_ONNX_MODEL_PATH_ENV, EMBEDDING_RUNTIME_ENV, EMBEDDING_RUNTIME_ONNX,
    EMBEDDING_TOKENIZER_VOCAB_PATH_ENV,
};

const BOOTSTRAP_MODEL_URL: &str =
    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx";
const BOOTSTRAP_VOCAB_URL: &str =
    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/vocab.txt";
const BOOTSTRAP_MODEL_ID: &str = "all-MiniLM-L6-v2";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapResult {
    pub model_path: std::path::PathBuf,
    pub vocab_path: std::path::PathBuf,
    pub manifest_path: std::path::PathBuf,
    pub model_size_bytes: u64,
    pub vocab_size_bytes: u64,
    pub vocab_token_count: usize,
}

/// Downloads the all-MiniLM-L6-v2 ONNX model and tokenizer vocabulary from Hugging Face,
/// writes the `.identity.json` manifest, and returns the resulting paths.
///
/// Uses the system `curl.exe` for downloads. Existing files are not re-downloaded.
pub fn bootstrap_onnx_artifact(
    model_dir: &std::path::Path,
) -> Result<BootstrapResult, std::io::Error> {
    std::fs::create_dir_all(model_dir)?;

    let model_path = model_dir.join("model.onnx");
    let vocab_path = model_dir.join("vocab.txt");

    // Download model if it doesn't already exist
    if !model_path.exists() {
        let temp_model = model_dir.join("model.onnx.tmp");
        download_file(BOOTSTRAP_MODEL_URL, &temp_model)?;
        std::fs::rename(&temp_model, &model_path)?;
    }

    // Download vocab if it doesn't already exist
    if !vocab_path.exists() {
        let temp_vocab = model_dir.join("vocab.txt.tmp");
        download_file(BOOTSTRAP_VOCAB_URL, &temp_vocab)?;
        std::fs::rename(&temp_vocab, &vocab_path)?;
    }

    // Validate the downloaded model and write manifest if needed
    let artifact = embedding_artifact_health_for_model_path(&model_path);

    if artifact.exists && artifact.is_file && artifact.has_onnx_extension {
        let manifest_path = embedding_manifest_path(&model_path);
        if !manifest_path.exists() {
            write_embedding_manifest(&model_path, BOOTSTRAP_MODEL_ID, false)?;
        }
    }

    // Re-check after manifest write
    let artifact = embedding_artifact_health_for_model_path(&model_path);
    if !artifact.is_ready() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "downloaded ONNX artifact is not ready: status={} path={}",
                artifact.status,
                artifact.path.as_deref().unwrap_or("unknown")
            ),
        ));
    }

    let model_size_bytes = std::fs::metadata(&model_path)?.len();
    let vocab_size_bytes = std::fs::metadata(&vocab_path)?.len();
    let tokenizer = tokenizer_health_for_vocab_path(&vocab_path);
    let vocab_token_count = tokenizer.token_count.unwrap_or(0);

    if tokenizer.status != "ready" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "downloaded tokenizer vocab is not healthy: status={}",
                tokenizer.status
            ),
        ));
    }

    Ok(BootstrapResult {
        model_path,
        vocab_path,
        manifest_path: embedding_manifest_path(&model_dir.join("model.onnx")),
        model_size_bytes,
        vocab_size_bytes,
        vocab_token_count,
    })
}

/// Print environment variable instructions for enabling the ONNX runtime.
pub fn print_bootstrap_guidance(result: &BootstrapResult) {
    println!("ONNX embedding artifact bootstrapped successfully.");
    println!();
    println!("  model  : {}", result.model_path.display());
    println!("  vocab  : {}", result.vocab_path.display());
    println!("  manifest: {}", result.manifest_path.display());
    println!("  model size : {} bytes", result.model_size_bytes);
    println!(
        "  vocab size : {} bytes ({} tokens)",
        result.vocab_size_bytes, result.vocab_token_count
    );
    println!();
    println!("To enable the ONNX embedding runtime, complete these steps:");
    println!();
    println!("1. Download onnxruntime.dll (Windows x64):");
    println!("   https://github.com/microsoft/onnxruntime/releases");
    println!("   (Download the DirectML or CPU package, extract onnxruntime.dll)");
    println!();
    println!("2. Set these environment variables:");
    println!();
    println!(
        "   set {}=\"{}\"",
        EMBEDDING_ONNX_MODEL_PATH_ENV,
        result.model_path.display()
    );
    println!(
        "   set {}=\"{}\"",
        EMBEDDING_TOKENIZER_VOCAB_PATH_ENV,
        result.vocab_path.display()
    );
    println!(
        "   set {}=<path-to-onnxruntime.dll>",
        EMBEDDING_ONNX_DYLIB_PATH_ENV
    );
    println!("   set {EMBEDDING_RUNTIME_ENV}={EMBEDDING_RUNTIME_ONNX}");
    println!();
    println!("3. Build with the ONNX feature flag:");
    println!("   cargo build --release -p identityd --features onnx-runtime");
    println!();
    println!("4. Verify with:");
    println!("   identityd doctor");
    println!();
    println!("  Expected output: phase1_embedding_runtime=onnx");
}

fn download_file(url: &str, dest: &std::path::Path) -> Result<(), std::io::Error> {
    let status = std::process::Command::new("curl.exe")
        .args([
            "-L",
            "-f",
            "--retry",
            "3",
            "--connect-timeout",
            "30",
            "--max-time",
            "600",
            "-o",
        ])
        .arg(dest)
        .arg(url)
        .status()
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("failed to run curl.exe for model download: {error}"),
            )
        })?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(std::io::Error::other(format!(
            "curl.exe exited with code {code}; download may have failed"
        )));
    }

    Ok(())
}
