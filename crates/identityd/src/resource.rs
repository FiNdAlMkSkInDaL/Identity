#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessResourceProbe {
    pub working_set_bytes: u64,
    pub pagefile_bytes: u64,
}

pub const IDLE_MEMORY_TARGET_BYTES: u64 = 35 * 1024 * 1024;

pub fn current_process_resources() -> Option<ProcessResourceProbe> {
    current_process_resources_platform()
}

pub fn memory_budget_status(working_set_bytes: Option<u64>) -> &'static str {
    match working_set_bytes {
        Some(bytes) if bytes <= IDLE_MEMORY_TARGET_BYTES => "within-budget",
        Some(_) => "over-budget",
        None => "unavailable",
    }
}

#[cfg(windows)]
fn current_process_resources_platform() -> Option<ProcessResourceProbe> {
    use std::ffi::c_void;

    type Bool = i32;
    type Dword = u32;
    type Handle = *mut c_void;

    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: Dword,
        page_fault_count: Dword,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> Handle;
    }

    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: Handle,
            counters: *mut ProcessMemoryCounters,
            size: Dword,
        ) -> Bool;
    }

    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as Dword,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };

    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };

    if ok == 0 {
        None
    } else {
        Some(ProcessResourceProbe {
            working_set_bytes: counters.working_set_size as u64,
            pagefile_bytes: counters.pagefile_usage as u64,
        })
    }
}

#[cfg(not(windows))]
fn current_process_resources_platform() -> Option<ProcessResourceProbe> {
    None
}

#[cfg(test)]
mod tests {
    use super::{memory_budget_status, IDLE_MEMORY_TARGET_BYTES};

    #[test]
    fn memory_budget_status_tracks_target() {
        assert_eq!(memory_budget_status(None), "unavailable");
        assert_eq!(
            memory_budget_status(Some(IDLE_MEMORY_TARGET_BYTES)),
            "within-budget"
        );
        assert_eq!(
            memory_budget_status(Some(IDLE_MEMORY_TARGET_BYTES + 1)),
            "over-budget"
        );
    }
}
