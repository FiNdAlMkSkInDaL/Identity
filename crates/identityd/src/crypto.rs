use std::fmt;

pub const PROTECTED_PREFIX: &str = "identity-protected:v1:";

#[cfg(windows)]
#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[derive(Debug)]
pub enum CryptoError {
    Decode(String),
    Platform(String),
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(reason) => write!(f, "protected content decode failed: {reason}"),
            Self::Platform(reason) => write!(f, "local content protection failed: {reason}"),
        }
    }
}

impl std::error::Error for CryptoError {}

pub fn protect_text(plaintext: &str) -> Result<String, CryptoError> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }

    let protected = protect_bytes(plaintext.as_bytes())?;
    Ok(format!("{PROTECTED_PREFIX}{}", hex_encode(&protected)))
}

pub fn unprotect_text(stored: &str) -> Result<String, CryptoError> {
    let Some(encoded) = stored.strip_prefix(PROTECTED_PREFIX) else {
        return Ok(stored.to_string());
    };

    let protected = hex_decode(encoded)?;
    let plaintext = unprotect_bytes(&protected)?;

    String::from_utf8(plaintext).map_err(|error| CryptoError::Decode(error.to_string()))
}

pub fn is_protected_text(stored: &str) -> bool {
    stored.starts_with(PROTECTED_PREFIX)
}

pub fn protection_backend() -> &'static str {
    protection_backend_platform()
}

#[cfg(windows)]
fn protection_backend_platform() -> &'static str {
    "windows-dpapi"
}

#[cfg(not(windows))]
fn protection_backend_platform() -> &'static str {
    "development-plaintext-fallback"
}

#[cfg(windows)]
fn protect_bytes(plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            p_data_in: *mut DataBlob,
            sz_data_descr: *const u16,
            p_optional_entropy: *mut DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
    }

    let mut input = DataBlob {
        cb_data: plaintext.len() as u32,
        pb_data: plaintext.as_ptr() as *mut u8,
    };
    let mut output = DataBlob {
        cb_data: 0,
        pb_data: null_mut(),
    };

    let ok = unsafe {
        CryptProtectData(
            &mut input,
            std::ptr::null(),
            null_mut(),
            null_mut(),
            null_mut(),
            0,
            &mut output,
        )
    };

    if ok == 0 {
        return Err(CryptoError::Platform(
            std::io::Error::last_os_error().to_string(),
        ));
    }

    take_local_allocated_blob(output)
}

#[cfg(windows)]
fn unprotect_bytes(protected: &[u8]) -> Result<Vec<u8>, CryptoError> {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptUnprotectData(
            p_data_in: *mut DataBlob,
            ppsz_data_descr: *mut *mut u16,
            p_optional_entropy: *mut DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
    }

    let mut input = DataBlob {
        cb_data: protected.len() as u32,
        pb_data: protected.as_ptr() as *mut u8,
    };
    let mut output = DataBlob {
        cb_data: 0,
        pb_data: null_mut(),
    };

    let ok = unsafe {
        CryptUnprotectData(
            &mut input,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            0,
            &mut output,
        )
    };

    if ok == 0 {
        return Err(CryptoError::Platform(
            std::io::Error::last_os_error().to_string(),
        ));
    }

    take_local_allocated_blob(output)
}

#[cfg(windows)]
fn take_local_allocated_blob(blob: DataBlob) -> Result<Vec<u8>, CryptoError> {
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(h_mem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    if blob.pb_data.is_null() {
        return Ok(Vec::new());
    }

    let bytes = unsafe { std::slice::from_raw_parts(blob.pb_data, blob.cb_data as usize).to_vec() };
    unsafe {
        LocalFree(blob.pb_data.cast::<std::ffi::c_void>());
    }

    Ok(bytes)
}

#[cfg(not(windows))]
fn protect_bytes(plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    Ok(plaintext.to_vec())
}

#[cfg(not(windows))]
fn unprotect_bytes(protected: &[u8]) -> Result<Vec<u8>, CryptoError> {
    Ok(protected.to_vec())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }

    output
}

fn hex_decode(input: &str) -> Result<Vec<u8>, CryptoError> {
    if !input.len().is_multiple_of(2) {
        return Err(CryptoError::Decode("hex length must be even".to_string()));
    }

    let mut bytes = Vec::with_capacity(input.len() / 2);
    for pair in input.as_bytes().chunks_exact(2) {
        let high = hex_value(pair[0])?;
        let low = hex_value(pair[1])?;
        bytes.push((high << 4) | low);
    }

    Ok(bytes)
}

fn hex_value(byte: u8) -> Result<u8, CryptoError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(CryptoError::Decode("invalid hex digit".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::{is_protected_text, protect_text, unprotect_text};

    #[test]
    fn protects_and_unprotects_text() {
        let protected = protect_text("local private capture").unwrap();

        assert!(is_protected_text(&protected));
        assert_ne!(protected, "local private capture");
        assert_eq!(unprotect_text(&protected).unwrap(), "local private capture");
    }

    #[test]
    fn reads_legacy_plaintext_without_migration() {
        assert!(!is_protected_text("legacy local text"));
        assert_eq!(
            unprotect_text("legacy local text").unwrap(),
            "legacy local text"
        );
    }
}
