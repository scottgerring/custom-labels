#[cfg(target_os = "linux")]
use super::encoding;
#[cfg(target_os = "linux")]
use super::model::{Error, ProcessContext, Result, PROCESS_CTX_VERSION, SIGNATURE};

#[cfg(target_os = "linux")]
use std::ptr;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, fence, Ordering};
#[cfg(target_os = "linux")]
use libc::timespec;

use tracing::info;

#[cfg(target_os = "linux")]
use std::sync::Mutex;

#[cfg(target_os = "linux")]
use libc::{
    c_void, close, ftruncate, madvise, mmap, munmap, prctl, sysconf, MADV_DONTFORK, MAP_ANONYMOUS,
    MAP_FAILED, MAP_PRIVATE, MAP_SHARED, PROT_READ, PROT_WRITE, _SC_PAGESIZE,
};

// prctl constants for naming anonymous mappings (Linux 5.17+)
#[cfg(target_os = "linux")]
const PR_SET_VMA: i32 = 0x53564d41;
#[cfg(target_os = "linux")]
const PR_SET_VMA_ANON_NAME: u64 = 0;

// memfd_create flags
#[cfg(target_os = "linux")]
const MFD_CLOEXEC: libc::c_uint = 0x0001;
#[cfg(target_os = "linux")]
const MFD_ALLOW_SEALING: libc::c_uint = 0x0002;
#[cfg(target_os = "linux")]
const MFD_NOEXEC_SEAL: libc::c_uint = 0x0008; // Linux 6.3+

/// Wrapper for memfd_create syscall
#[cfg(target_os = "linux")]
unsafe fn memfd_create(name: *const libc::c_char, flags: libc::c_uint) -> libc::c_int {
    libc::syscall(libc::SYS_memfd_create, name, flags) as libc::c_int
}

/// The header structure written at the start of the mapping.
/// This must match the C implementation exactly.
#[cfg(target_os = "linux")]
#[repr(C, packed)]
struct MappingHeader {
    signature: [u8; 8],
    version: u32,
    payload_size: u32,
    monotonic_published_at_ns: u64,
    payload_ptr: *const u8,
}

/// Get the mapping size (1 page, as per PR #34)
#[cfg(target_os = "linux")]
fn mapping_size() -> Result<usize> {
    let page_size = unsafe { sysconf(_SC_PAGESIZE) };
    if page_size < 4096 {
        return Err(Error::MappingFailed("failed to get page size".to_string()));
    }
    Ok(page_size as usize)
}

/// Get a monotonic timestamp in nanoseconds from CLOCK_BOOTTIME.
/// Returns 0 on failure (callers treat 0 as "not yet ready").
#[cfg(target_os = "linux")]
fn monotonic_now_ns() -> u64 {
    let mut ts: timespec = unsafe { std::mem::zeroed() };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) } == -1 {
        return 0;
    }
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Result of create_mapping: the mapping pointer.
/// Any memfd file descriptor is closed immediately after mmap —
/// the mapping keeps the underlying resource alive.
#[cfg(target_os = "linux")]
struct MappingResult {
    mapping: *mut c_void,
}

/// Create the memory mapping, trying memfd first, then falling back to anonymous.
#[cfg(target_os = "linux")]
fn create_mapping(size: usize) -> Result<MappingResult> {
    let memfd_name = b"OTEL_CTX\0";

    // Try memfd_create with MFD_NOEXEC_SEAL first (Linux 6.3+)
    let mut fd = unsafe {
        memfd_create(
            memfd_name.as_ptr() as *const libc::c_char,
            MFD_CLOEXEC | MFD_ALLOW_SEALING | MFD_NOEXEC_SEAL,
        )
    };

    if fd < 0 {
        // Retry without MFD_NOEXEC_SEAL for older kernels
        fd = unsafe {
            memfd_create(
                memfd_name.as_ptr() as *const libc::c_char,
                MFD_CLOEXEC | MFD_ALLOW_SEALING,
            )
        };
    }

    if fd >= 0 {
        // Set the size of the memfd
        if unsafe { ftruncate(fd, size as libc::off_t) } == -1 {
            info!("ftruncate failed on memfd, falling back to anonymous mapping");
            unsafe { close(fd) };
        } else {
            // Map the memfd - use MAP_SHARED so it shows in /proc/pid/maps
            let mapping = unsafe {
                mmap(
                    ptr::null_mut(),
                    size,
                    PROT_READ | PROT_WRITE,
                    MAP_SHARED,
                    fd,
                    0,
                )
            };

            if mapping != MAP_FAILED {
                // Close fd immediately — the mapping keeps the resource alive
                unsafe { close(fd) };
                info!("Created memfd mapping (will appear as /memfd:OTEL_CTX in maps)");
                return Ok(MappingResult { mapping });
            }

            // mmap failed, close fd and fall through to anonymous
            info!("mmap of memfd failed, falling back to anonymous mapping");
            unsafe { close(fd) };
        }
    } else {
        info!("memfd_create not available, using anonymous mapping");
    }

    // Fallback to anonymous mapping
    let mapping = unsafe {
        mmap(
            ptr::null_mut(),
            size,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )
    };

    if mapping == MAP_FAILED {
        return Err(Error::MappingFailed("mmap failed".to_string()));
    }

    info!("Created anonymous mapping");
    Ok(MappingResult { mapping })
}

/// Internal handle holding the mapping state.
#[cfg(target_os = "linux")]
struct ProcessContextHandle {
    mapping: *mut c_void,
    mapping_size: usize,
    payload: Vec<u8>,
    publisher_pid: libc::pid_t,
}

// Safety: the handle is only accessed under the global mutex.
#[cfg(target_os = "linux")]
unsafe impl Send for ProcessContextHandle {}

#[cfg(target_os = "linux")]
static PROCESS_CONTEXT: Mutex<Option<ProcessContextHandle>> = Mutex::new(None);

/// Get an AtomicU64 reference to the timestamp field in the mapping header.
///
/// Safety: the caller must ensure `header` points to a valid MappingHeader
/// and that the `monotonic_published_at_ns` field is aligned to 8 bytes.
#[cfg(target_os = "linux")]
unsafe fn ts_atomic(header: *mut MappingHeader) -> &'static AtomicU64 {
    let ts_ptr = ptr::addr_of_mut!((*header).monotonic_published_at_ns);
    AtomicU64::from_ptr(ts_ptr.cast())
}

/// Writer for publishing process context.
///
/// This creates a memory mapping that can be discovered by external readers
/// via `/proc/self/maps`. The implementation tries memfd first (which shows
/// as `/memfd:OTEL_CTX` in maps), falling back to anonymous mapping with
/// prctl naming (`[anon:OTEL_CTX]`).
///
/// The mapping stays writable to support in-place updates via `update()`.
///
/// All operations are thread-safe via an internal global mutex.
#[cfg(target_os = "linux")]
pub struct ProcessContextWriter {
    _private: (),
}

#[cfg(target_os = "linux")]
impl ProcessContextWriter {
    /// Publish a process context.
    ///
    /// This creates a memory mapping containing the encoded context.
    /// The mapping can be discovered by external readers via `/proc/self/maps`.
    pub fn publish(ctx: &ProcessContext) -> Result<Self> {
        let mut guard = PROCESS_CONTEXT.lock().unwrap();

        info!("Publishing process context");

        // Get timestamp first — fail early before allocating resources
        let ts = monotonic_now_ns();
        if ts == 0 {
            return Err(Error::MappingFailed(
                "monotonic clock returned zero".to_string(),
            ));
        }

        let size = mapping_size()?;

        // Encode the payload
        let payload = encoding::encode(ctx)?;
        info!(payload_size = payload.len(), "Encoded process context payload");

        // Create mapping (memfd preferred, fallback to anonymous)
        let MappingResult { mapping } = create_mapping(size)?;
        info!(mapping_addr = ?mapping, mapping_size = size, "Created mapping");

        // Set MADV_DONTFORK so children don't inherit this mapping
        if unsafe { madvise(mapping, size, MADV_DONTFORK) } == -1 {
            unsafe { munmap(mapping, size) };
            return Err(Error::MappingFailed(
                "madvise MADV_DONTFORK failed".to_string(),
            ));
        }

        // Write header fields (signature, version, payload_size, payload)
        // but NOT monotonic_published_at_ns yet — it is the validity gate.
        let header = mapping as *mut MappingHeader;
        unsafe {
            ptr::copy_nonoverlapping(SIGNATURE.as_ptr(), (*header).signature.as_mut_ptr(), 8);
            (*header).version = PROCESS_CTX_VERSION;
            (*header).payload_size = payload.len() as u32;
            (*header).monotonic_published_at_ns = 0; // not yet valid
            (*header).payload_ptr = payload.as_ptr();
        }

        // Memory fence to ensure all header fields are visible before the timestamp
        fence(Ordering::SeqCst);

        // Atomically write monotonic_published_at_ns to signal the mapping is valid
        unsafe {
            ts_atomic(header).store(ts, Ordering::Relaxed);
        }
        info!("Wrote header with signature OTEL_CTX");

        // Try to name the mapping so outside readers can find it
        let name = b"OTEL_CTX\0";
        let prctl_result = unsafe {
            prctl(
                PR_SET_VMA,
                PR_SET_VMA_ANON_NAME,
                mapping,
                size,
                name.as_ptr(),
            )
        };
        if prctl_result == 0 {
            info!("Named mapping [anon:OTEL_CTX]");
        } else {
            info!("Could not name mapping (older kernel), will be discoverable by signature");
        }

        info!("Process context published successfully");

        *guard = Some(ProcessContextHandle {
            mapping,
            mapping_size: size,
            payload,
            publisher_pid: unsafe { libc::getpid() },
        });

        Ok(Self { _private: () })
    }

    /// Update the process context in place.
    ///
    /// This uses the atomic update protocol:
    /// 1. Atomically zero `monotonic_published_at_ns` to signal update in progress
    /// 2. Memory fence
    /// 3. Update payload pointer and size
    /// 4. Memory fence
    /// 5. Atomically write new monotonic timestamp (must be strictly after previous value)
    ///
    /// Readers should check if `monotonic_published_at_ns == 0` and retry/skip.
    pub fn update(&mut self, ctx: &ProcessContext) -> Result<()> {
        let mut guard = PROCESS_CONTEXT.lock().unwrap();
        let handle = guard
            .as_mut()
            .ok_or_else(|| Error::MappingFailed("no published context to update".to_string()))?;

        info!("Updating process context in place");

        // Encode new payload
        let new_payload = encoding::encode(ctx)?;
        info!(payload_size = new_payload.len(), "Encoded new payload");

        let header = handle.mapping as *mut MappingHeader;

        // Read the current timestamp atomically before zeroing
        let prev_ts = unsafe { ts_atomic(header).swap(0, Ordering::Relaxed) };

        // Step 2: Memory fence
        fence(Ordering::SeqCst);

        // Step 3: Update payload - store new payload and update header
        handle.payload = new_payload;
        unsafe {
            (*header).payload_ptr = handle.payload.as_ptr();
            (*header).payload_size = handle.payload.len() as u32;
        }

        // Step 4: Memory fence
        fence(Ordering::SeqCst);

        // Step 5: Atomically write new monotonic timestamp
        let mut ts = monotonic_now_ns();
        if ts <= prev_ts {
            ts = prev_ts + 1;
        }
        unsafe {
            ts_atomic(header).store(ts, Ordering::Relaxed);
        }

        info!("Process context updated successfully");
        Ok(())
    }

    /// Drop/unpublish the current process context.
    ///
    /// This unmaps the memory and frees resources.
    pub fn drop_context(&mut self) -> Result<()> {
        let mut guard = PROCESS_CONTEXT.lock().unwrap();
        if let Some(handle) = guard.take() {
            // Only unmap if we're in the same process that created it
            let current_pid = unsafe { libc::getpid() };
            if current_pid == handle.publisher_pid {
                if unsafe { munmap(handle.mapping, handle.mapping_size) } == -1 {
                    return Err(Error::MappingFailed("munmap failed".to_string()));
                }
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for ProcessContextWriter {
    fn drop(&mut self) {
        let _ = self.drop_context();
    }
}

// Non-Linux stub implementation
#[cfg(not(target_os = "linux"))]
use super::model::{Error, ProcessContext, Result};

#[cfg(not(target_os = "linux"))]
pub struct ProcessContextWriter;

#[cfg(not(target_os = "linux"))]
impl ProcessContextWriter {
    pub fn publish(_ctx: &ProcessContext) -> Result<Self> {
        info!("Process context publishing not supported on this platform (Linux only)");
        Err(Error::PlatformNotSupported)
    }

    pub fn update(&mut self, _ctx: &ProcessContext) -> Result<()> {
        Err(Error::PlatformNotSupported)
    }

    pub fn drop_context(&mut self) -> Result<()> {
        Err(Error::PlatformNotSupported)
    }
}
