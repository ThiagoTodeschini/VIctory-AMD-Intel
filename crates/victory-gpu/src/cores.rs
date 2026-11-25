//! Where the layer's own threads run on a processor with two kinds of
//! cores (Intel's hybrid processors: performance and efficient cores).
//!
//! Ported from fxceless/victory-fork, commit 0b0074e (`framelog.rs`:
//! `cpu_sets`, `keep_on_fast_cores`, `keep_on_slow_cores`), licensed
//! GPL-3.0-or-later like this fork. Here the processor sets are read once
//! and kept.
//!
//! The title pins its own threads as on the console, whose eight cores are
//! alike: on an i7-14700KF fxceless found 31 of them tied to logical
//! processors 0-13, the fast cores. The threads the layer starts are free
//! to run anywhere, slow cores included. A set of processors selected for
//! a thread is a preference the scheduler keeps unless nothing of it is
//! free.

use std::sync::OnceLock;

/// The system's processor sets of group 0: each one's id, its logical
/// processor and its efficiency class (higher is faster).
fn cpu_sets() -> &'static [(u32, u8, u8)] {
    static SETS: OnceLock<Vec<(u32, u8, u8)>> = OnceLock::new();
    SETS.get_or_init(|| {
        use windows_sys::Win32::System::SystemInformation::{
            GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut bytes = 0u32;
        // SAFETY: a size query, then the call into a buffer of that size.
        unsafe {
            GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut bytes, GetCurrentProcess(), 0)
        };
        let mut buffer = vec![0u64; (bytes as usize).div_ceil(8).max(1)];
        // SAFETY: as above; the buffer holds `bytes` bytes, aligned for the records.
        let ok = unsafe {
            GetSystemCpuSetInformation(
                buffer.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
                GetCurrentProcess(),
                0,
            )
        };
        let mut sets = Vec::new();
        let mut at = 0usize;
        while ok != 0 && at + size_of::<SYSTEM_CPU_SET_INFORMATION>() <= bytes as usize {
            // SAFETY: a record inside the buffer; each says its own size.
            let info = unsafe {
                &*(buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(at)
                    .cast::<SYSTEM_CPU_SET_INFORMATION>())
            };
            if info.Size == 0 {
                break;
            }
            // SAFETY: the record's type is CpuSet (the only one there is).
            let set = unsafe { &info.Anonymous.CpuSet };
            if set.Group == 0 {
                sets.push((set.Id, set.LogicalProcessorIndex, set.EfficiencyClass));
            }
            at += info.Size as usize;
        }
        sets
    })
}

/// The set ids of the fastest cores and of the others; both empty where
/// all cores are alike or nothing is known.
fn kinds() -> &'static (Vec<u32>, Vec<u32>) {
    static KINDS: OnceLock<(Vec<u32>, Vec<u32>)> = OnceLock::new();
    KINDS.get_or_init(|| {
        let sets = cpu_sets();
        let best = sets.iter().map(|s| s.2).max().unwrap_or(0);
        let fast: Vec<u32> = sets.iter().filter(|s| s.2 == best).map(|s| s.0).collect();
        let slow: Vec<u32> = sets.iter().filter(|s| s.2 < best).map(|s| s.0).collect();
        if fast.is_empty() || slow.is_empty() {
            return (Vec::new(), Vec::new());
        }
        tracing::info!(
            fast = fast.len(),
            slow = slow.len(),
            "a processor with two kinds of cores: its logical processors"
        );
        (fast, slow)
    })
}

/// Selects the sets `ids` for the calling thread; false if refused.
fn select(ids: &[u32]) -> bool {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadSelectedCpuSets};
    // SAFETY: our own thread and a list of the system's own set ids.
    unsafe { SetThreadSelectedCpuSets(GetCurrentThread(), ids.as_ptr(), ids.len() as u32) != 0 }
}

/// Asks the system to run the calling thread on the fast cores only, where
/// the processor has two kinds (`VICTORY_FAST_CORES=0`: not asked). For a
/// thread the frame waits for: fxceless found its submit thread on a slow
/// core in 1,529 looks of 24,021, and in none once it asked.
pub fn keep_on_fast_cores(what: &str) {
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("VICTORY_FAST_CORES").is_none_or(|v| v != "0")) {
        return;
    }
    let (fast, slow) = kinds();
    if fast.is_empty() {
        return;
    }
    if select(fast) {
        tracing::info!(
            what,
            fast = fast.len(),
            of = fast.len() + slow.len(),
            "the thread is kept on the fast cores (VICTORY_FAST_CORES=0: not)"
        );
    } else {
        tracing::warn!(
            what,
            error = %std::io::Error::last_os_error(),
            "the thread could not be kept on the fast cores"
        );
    }
}

/// Asks the system to run the calling thread on the slow cores only, where
/// the processor has two kinds and at least eight slow logical processors:
/// for a helper that must not take a fast core from the title's threads.
/// True if it was asked and granted. fxceless saw about ten of the layer's
/// decompression workers busy in the opening cutscene's streaming while the
/// title's recording calls took three times as long.
pub fn keep_on_slow_cores(what: &str) -> bool {
    let (_, slow) = kinds();
    if slow.len() < 8 {
        return false;
    }
    let ok = select(slow);
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::info!(
            what,
            slow = slow.len(),
            ok,
            "helper threads are kept on the slow cores: the first one"
        );
    });
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_set_is_of_one_kind() {
        let sets = cpu_sets();
        assert!(!sets.is_empty(), "the system lists no processor sets");
        let (fast, slow) = kinds();
        if !fast.is_empty() {
            assert_eq!(fast.len() + slow.len(), sets.len());
            assert!(fast.iter().all(|id| !slow.contains(id)));
        }
    }
}
