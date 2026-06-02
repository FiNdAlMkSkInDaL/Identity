use std::fmt;

pub const MAX_CAPTURE_CONTENT_BYTES: usize = 1024 * 1024;
pub const MAX_CAPTURE_SOURCE_BYTES: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestSafetyError {
    BlockedSource(String),
    BlockedContent(String),
}

impl fmt::Display for IngestSafetyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlockedSource(reason) => write!(f, "blocked capture source: {reason}"),
            Self::BlockedContent(reason) => write!(f, "blocked capture content: {reason}"),
        }
    }
}

impl std::error::Error for IngestSafetyError {}

pub fn validate_capture(source: &str, content: &str) -> Result<(), IngestSafetyError> {
    validate_source(source)?;
    validate_content(content)?;
    Ok(())
}

fn validate_source(source: &str) -> Result<(), IngestSafetyError> {
    if source.as_bytes().len() > MAX_CAPTURE_SOURCE_BYTES {
        return Err(IngestSafetyError::BlockedSource(
            "source label exceeds 2048-byte budget".to_string(),
        ));
    }

    let lower = source.to_ascii_lowercase();

    if lower.contains(".env")
        || lower.ends_with("id_rsa")
        || lower.ends_with("id_ed25519")
        || lower.ends_with("id_ecdsa")
        || lower.ends_with("id_dsa")
        || lower.ends_with("credentials")
        || lower.ends_with("credentials.json")
        || lower.ends_with("known_hosts")
        || lower.ends_with("authorized_keys")
        || lower.ends_with("secrets.json")
        || lower.ends_with("secrets.toml")
        || lower.ends_with("secrets.yaml")
        || lower.ends_with("secrets.yml")
        || lower.contains("/.ssh/")
        || lower.contains("\\.ssh\\")
        || lower.contains("/.aws/")
        || lower.contains("\\.aws\\")
        || lower.contains("/.azure/")
        || lower.contains("\\.azure\\")
        || lower.contains("/.gnupg/")
        || lower.contains("\\.gnupg\\")
    {
        return Err(IngestSafetyError::BlockedSource(
            "secret-bearing file path".to_string(),
        ));
    }

    Ok(())
}

fn validate_content(content: &str) -> Result<(), IngestSafetyError> {
    if content.as_bytes().len() > MAX_CAPTURE_CONTENT_BYTES {
        return Err(IngestSafetyError::BlockedContent(
            "capture exceeds 1MB transit budget".to_string(),
        ));
    }

    let lower = content.to_ascii_lowercase();

    if lower.contains("-----begin openssh private key-----")
        || lower.contains("-----begin rsa private key-----")
        || lower.contains("-----begin ec private key-----")
        || lower.contains("-----begin dsa private key-----")
        || lower.contains("-----begin private key-----")
        || lower.contains("-----begin pgp private key block-----")
    {
        return Err(IngestSafetyError::BlockedContent(
            "private key material".to_string(),
        ));
    }

    for marker in [
        "aws_secret_access_key",
        "private_key",
        "client_secret",
        "system_password",
        "database_password",
        "db_password",
        "password=",
        "api_key=",
        "api-key:",
        "apikey=",
        "secret_key=",
        "authorization: bearer ",
        "bearer_token",
        "access_token=",
        "refresh_token=",
        "session_token=",
        "stripe_secret",
    ] {
        if lower.contains(marker) {
            return Err(IngestSafetyError::BlockedContent(format!(
                "credential marker `{marker}`"
            )));
        }
    }

    if contains_known_secret_prefix(content) {
        return Err(IngestSafetyError::BlockedContent(
            "known secret token prefix".to_string(),
        ));
    }

    if contains_luhn_number(content) {
        return Err(IngestSafetyError::BlockedContent(
            "payment-card-like number".to_string(),
        ));
    }

    if lower.contains("routing number") || lower.contains("routing_number") {
        return Err(IngestSafetyError::BlockedContent(
            "bank-routing marker".to_string(),
        ));
    }

    if (lower.contains("latitude") && lower.contains("longitude"))
        || lower.contains("gps_coordinates")
    {
        return Err(IngestSafetyError::BlockedContent(
            "precise-location marker".to_string(),
        ));
    }

    Ok(())
}

fn contains_known_secret_prefix(input: &str) -> bool {
    input
        .split(|character: char| {
            character.is_ascii_whitespace()
                || matches!(character, '"' | '\'' | '`' | '=' | ':' | ',')
        })
        .any(|token| {
            let lower = token.to_ascii_lowercase();
            lower.starts_with("ghp_")
                || lower.starts_with("github_pat_")
                || lower.starts_with("sk-")
                || lower.starts_with("xoxb-")
                || lower.starts_with("xoxp-")
                || lower.starts_with("xoxa-")
                || lower.starts_with("akiaj")
                || lower.starts_with("akia")
                || lower.starts_with("asiaj")
                || lower.starts_with("asii")
        })
}

fn contains_luhn_number(input: &str) -> bool {
    let mut digits = String::with_capacity(19);

    for character in input.chars().chain(std::iter::once(' ')) {
        if character.is_ascii_digit() {
            if digits.len() < 19 {
                digits.push(character);
            }
            continue;
        }

        if digits.len() >= 13 && luhn_valid(&digits) {
            return true;
        }

        digits.clear();
    }

    false
}

fn luhn_valid(digits: &str) -> bool {
    let mut sum = 0;
    let mut double = false;

    for byte in digits.as_bytes().iter().rev() {
        let mut value = byte - b'0';

        if double {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }

        sum += u32::from(value);
        double = !double;
    }

    sum % 10 == 0
}

#[cfg(test)]
mod tests {
    use super::{validate_capture, MAX_CAPTURE_CONTENT_BYTES, MAX_CAPTURE_SOURCE_BYTES};

    #[test]
    fn blocks_secret_bearing_sources() {
        let error = validate_capture("filesystem:C:/Users/me/.env", "HELLO=world").unwrap_err();
        assert!(error.to_string().contains("secret-bearing"));

        assert!(validate_capture("filesystem:C:/Users/me/.ssh/id_ed25519", "plain text").is_err());
        assert!(validate_capture(
            "filesystem:C:/Users/me/AppData/Roaming/aws/credentials",
            "plain text"
        )
        .is_err());
        assert!(validate_capture("filesystem:C:/tmp/secrets.yaml", "plain text").is_err());
    }

    #[test]
    fn blocks_private_keys_and_credential_markers() {
        assert!(validate_capture("manual", "-----BEGIN OPENSSH PRIVATE KEY-----\nabc").is_err());
        assert!(validate_capture("manual", "-----BEGIN PGP PRIVATE KEY BLOCK-----\nabc").is_err());
        assert!(validate_capture("manual", "database_password=hunter2").is_err());
        assert!(validate_capture("manual", "Authorization: Bearer abcdef").is_err());
        assert!(validate_capture("manual", "refresh_token=abcdef").is_err());
    }

    #[test]
    fn blocks_known_secret_token_prefixes() {
        assert!(validate_capture("manual", "token ghp_abcd1234").is_err());
        assert!(validate_capture("manual", "api_key: sk-test-local").is_err());
        assert!(validate_capture("manual", "slack xoxb-123-456").is_err());
        assert!(validate_capture("manual", "aws AKIAJEXAMPLEKEY").is_err());
    }

    #[test]
    fn blocks_oversized_sources_and_content_before_persistence() {
        let oversized_source = "a".repeat(MAX_CAPTURE_SOURCE_BYTES + 1);
        assert!(validate_capture(&oversized_source, "plain text").is_err());

        let oversized_content = "a".repeat(MAX_CAPTURE_CONTENT_BYTES + 1);
        assert!(validate_capture("manual", &oversized_content).is_err());
    }

    #[test]
    fn blocks_card_like_numbers_and_location_markers() {
        assert!(validate_capture("manual", "card 4111111111111111").is_err());
        assert!(validate_capture("manual", "latitude=10 longitude=20").is_err());
    }

    #[test]
    fn allows_normal_local_notes() {
        validate_capture(
            "manual",
            "Identity should ingest local project notes and markdown safely.",
        )
        .unwrap();
    }
}
