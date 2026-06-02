use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdleError {
    Unavailable,
}

impl fmt::Display for IdleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => write!(f, "system idle telemetry is unavailable"),
        }
    }
}

impl std::error::Error for IdleError {}

pub fn is_idle_for(min_idle: Duration) -> Result<bool, IdleError> {
    let Some(idle) = idle_duration()? else {
        return Ok(true);
    };

    Ok(idle >= min_idle)
}

#[cfg(windows)]
pub fn idle_duration() -> Result<Option<Duration>, IdleError> {
    #[repr(C)]
    struct LastInputInfo {
        cb_size: u32,
        dw_time: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetLastInputInfo(plii: *mut LastInputInfo) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount64() -> u64;
    }

    let mut info = LastInputInfo {
        cb_size: std::mem::size_of::<LastInputInfo>() as u32,
        dw_time: 0,
    };

    let ok = unsafe { GetLastInputInfo(&mut info as *mut LastInputInfo) };
    if ok == 0 {
        return Err(IdleError::Unavailable);
    }

    let now = unsafe { GetTickCount64() };
    let last_input = u64::from(info.dw_time);
    let idle_ms = now.saturating_sub(last_input);

    Ok(Some(Duration::from_millis(idle_ms)))
}

#[cfg(not(windows))]
pub fn idle_duration() -> Result<Option<Duration>, IdleError> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::is_idle_for;
    use std::time::Duration;

    #[test]
    fn idle_check_is_non_fatal() {
        let result = is_idle_for(Duration::from_millis(0));
        assert!(result.is_ok());
    }
}
