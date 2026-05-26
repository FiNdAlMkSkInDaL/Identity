use std::fmt;

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
    let lower = source.to_ascii_lowercase();

    if lower.contains(".env")
        || lower.ends_with("id_rsa")
        || lower.ends_with("id_ed25519")
        || lower.ends_with("credentials")
        || lower.ends_with("credentials.json")
    {
        return Err(IngestSafetyError::BlockedSource(
            "secret-bearing file path".to_string(),
        ));
    }

    Ok(())
}

fn validate_content(content: &str) -> Result<(), IngestSafetyError> {
    let lower = content.to_ascii_lowercase();

    if lower.contains("-----begin openssh private key-----")
        || lower.contains("-----begin rsa private key-----")
        || lower.contains("-----begin private key-----")
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
        "secret_key=",
    ] {
        if lower.contains(marker) {
            return Err(IngestSafetyError::BlockedContent(format!(
                "credential marker `{marker}`"
            )));
        }
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
    use super::validate_capture;

    #[test]
    fn blocks_secret_bearing_sources() {
        let error = validate_capture("filesystem:C:/Users/me/.env", "HELLO=world").unwrap_err();
        assert!(error.to_string().contains("secret-bearing"));
    }

    #[test]
    fn blocks_private_keys_and_credential_markers() {
        assert!(validate_capture("manual", "-----BEGIN OPENSSH PRIVATE KEY-----\nabc").is_err());
        assert!(validate_capture("manual", "database_password=hunter2").is_err());
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
            "Sovereign should ingest local project notes and markdown safely.",
        )
        .unwrap();
    }
}
