//! Pultflut Updater - native library untuk libapp.so runtime patching
//!
//! API ini didesain kompatibel dengan Shorebird's libupdater.so, supaya
//! bisa di-drop-in ke aplikasi Flutter yang sudah punya custom engine
//! wrapper, atau dipanggil langsung dari Java/Kotlin via JNI.
//!
//! ## Fungsi utama
//!
//! - `shorebird_init()` — Inisialisasi dengan parameter dari Java
//! - `shorebird_update()` — Apply patch kalau ada, return status
//! - `shorebird_active_path()` — Path ke libapp.so yang harus di-load
//! - `shorebird_active_patch_number()` — Patch number yang aktif
//! - `shorebird_free_string()` — Free string yang dialokasikan library
//!
//! ## Format patch (cache layout)
//!
//! ```text
//! <cache_dir>/patches/<patch_number>/patch.bin   (bsdiff 4.x format)
//! <cache_dir>/patches/<patch_number>/libapp.so   (output, atomic rename)
//! ```
//!
//! Patch dengan directory name lexicographically terbesar yang diambil.

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::fs;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::Once;

use log::{debug, error, info, warn};
use qbsdiff::{Bsdiff, Bspatch};

#[cfg(target_os = "android")]
mod jni_shim;

thread_local! {
    static STATE: RefCell<Option<UpdaterState>> = const { RefCell::new(None) };
}

struct UpdaterState {
    /// /data/data/<package>/cache/shorebird_updater
    cache_dir: PathBuf,
    /// /data/data/<package>/lib (native lib dir)
    libapp_dir: PathBuf,
    /// Version asli dari bundled APK
    release_version: String,
    /// Path ke libapp.so yang harus di-load (bundled atau patched)
    active_path: PathBuf,
    /// Patch number yang aktif (None = no patch)
    active_patch_number: Option<String>,
}

static INIT_LOG: Once = Once::new();

fn init_logging() {
    INIT_LOG.call_once(|| {
        #[cfg(target_os = "android")]
        {
            use android_logger::Config;
            android_logger::init_once(
                Config::default()
                    .with_tag("PultflutUpdater")
                    .with_max_level(log::LevelFilter::Info),
            );
        }
        // On desktop (non-Android, non-test), logs go to logcat only
        // on Android. Tests use env_logger::builder() directly.
    });
}

/// Helper: jalankan closure dengan mutable reference ke state.
/// Returns None kalau state belum di-init.
fn with_state_mut<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut UpdaterState) -> R,
{
    STATE.with(|s| {
        let mut b = s.borrow_mut();
        b.as_mut().map(f)
    })
}

// =============================================================================
// C API
// =============================================================================

/// Initialize updater state. Called once at app startup.
///
/// # Safety
/// All pointers must be valid null-terminated C strings. Returns 0 on
/// success, non-zero on error.
#[no_mangle]
pub extern "C" fn shorebird_init(
    release_version: *const c_char,
    cache_dir: *const c_char,
    libapp_dir: *const c_char,
) -> i32 {
    init_logging();

    let release_version = unsafe {
        match CStr::from_ptr(release_version).to_str() {
            Ok(s) => s.to_owned(),
            Err(e) => {
                error!("release_version not valid UTF-8: {}", e);
                return 1;
            }
        }
    };

    let cache_dir = unsafe {
        match CStr::from_ptr(cache_dir).to_str() {
            Ok(s) => PathBuf::from(s),
            Err(e) => {
                error!("cache_dir not valid UTF-8: {}", e);
                return 1;
            }
        }
    };

    let libapp_dir = unsafe {
        match CStr::from_ptr(libapp_dir).to_str() {
            Ok(s) => PathBuf::from(s),
            Err(e) => {
                error!("libapp_dir not valid UTF-8: {}", e);
                return 1;
            }
        }
    };

    info!(
        "shorebird_init: version={}, cache={}, libapp={}",
        release_version,
        cache_dir.display(),
        libapp_dir.display()
    );

    // Default: no patch, use bundled libapp.so
    let active_path = libapp_dir.join("libapp.so");

    STATE.with(|s| {
        *s.borrow_mut() = Some(UpdaterState {
            cache_dir,
            libapp_dir,
            release_version,
            active_path,
            active_patch_number: None,
        });
    });

    0
}

/// Check for pending patch in cache, apply if found.
///
/// Patch layout: `<cache_dir>/patches/<patch_number>/patch.bin`
/// Output: `<cache_dir>/patches/<patch_number>/libapp.so`
///
/// Returns 0 on success (whether or not a patch was applied),
/// non-zero on error.
#[no_mangle]
pub extern "C" fn shorebird_update() -> i32 {
    init_logging();

    // Read paths first
    let (cache_dir, libapp_dir) = match with_state_mut(|s| (s.cache_dir.clone(), s.libapp_dir.clone())) {
        Some(p) => p,
        None => {
            error!("shorebird_update: state not initialized");
            return 1;
        }
    };

    let patches_dir = cache_dir.join("patches");
    if !patches_dir.exists() {
        info!("No patches dir at {}", patches_dir.display());
        return 0;
    }

    // Find latest patch (lexicographic max of directory names)
    let latest_patch = match find_latest_patch(&patches_dir) {
        Some(p) => p,
        None => {
            info!("No patches in {}", patches_dir.display());
            return 0;
        }
    };

    let patch_number = latest_patch
        .file_name()
        .and_then(|n| n.to_str())
        .map(String::from)
        .unwrap_or_else(|| "unknown".to_string());

    let patch_file = latest_patch.join("patch.bin");
    let full_libapp = latest_patch.join("libapp.so");
    let tmp_output = latest_patch.join("libapp.so.tmp");
    let final_output = latest_patch.join("libapp.so");

    info!("Found latest patch: {} (#{})", latest_patch.display(), patch_number);

    // Two patch formats supported:
    // 1. Full file: just use the file in the patch dir directly
    // 2. bsdiff: apply patch to base libapp.so
    if full_libapp.exists() {
        info!("Patch #{} is a full libapp.so", patch_number);
        match verify_elf(&full_libapp) {
            Ok(size) => info!("Verified ELF ({} bytes)", size),
            Err(e) => {
                error!("Full libapp.so invalid: {}", e);
                return 1;
            }
        }
        // For full file, write to tmp then atomic rename to ensure
        // the final file is never half-written.
        if let Err(e) = fs::copy(&full_libapp, &tmp_output) {
            error!("Copy to tmp failed: {}", e);
            return 1;
        }
    } else if patch_file.exists() {
        info!("Patch #{} is a bsdiff ({})", patch_number, patch_file.display());
        let base_path = match find_libapp_base(&libapp_dir) {
            Some(p) => p,
            None => {
                let err = format!(
                    "Base libapp.so not found in: {}. On modern Android, the file may be embedded in the APK. Push a full libapp.so instead of a bsdiff patch.",
                    libapp_dir.display()
                );
                error!("{}", err);
                return 1;
            }
        };
        info!("Using base: {}", base_path.display());
        if let Err(e) = apply_bsdiff(&base_path, &patch_file, &tmp_output) {
            error!("Patch apply failed: {}", e);
            let _ = fs::remove_file(&tmp_output);
            return 1;
        }
    } else {
        error!("Patch dir has neither patch.bin nor libapp.so");
        return 1;
    }

    // Atomic rename
    if let Err(e) = atomic_rename(&tmp_output, &final_output) {
        error!("Atomic rename failed: {}", e);
        let _ = fs::remove_file(&tmp_output);
        return 1;
    }

    info!("Patch #{} applied: {}", patch_number, final_output.display());

    // Update state
    with_state_mut(|s| {
        s.active_path = final_output;
        s.active_patch_number = Some(patch_number);
    });

    0
}

/// Return path to the libapp.so that should be loaded.
/// Caller must NOT free this; use shorebird_free_string() instead.
///
/// Returns NULL on error.
#[no_mangle]
pub extern "C" fn shorebird_active_path() -> *const c_char {
    with_state_mut(|s| {
        let path = s.active_path.to_string_lossy().into_owned();
        match CString::new(path) {
            Ok(c) => c.into_raw() as *const c_char,
            Err(e) => {
                error!("active_path: NUL in path: {}", e);
                std::ptr::null()
            }
        }
    })
    .unwrap_or_else(|| {
        error!("active_path: state not initialized");
        std::ptr::null()
    })
}

/// Return patch number of active patch, or NULL if no patch active.
#[no_mangle]
pub extern "C" fn shorebird_active_patch_number() -> *const c_char {
    with_state_mut(|s| match &s.active_patch_number {
        Some(n) => match CString::new(n.as_str()) {
            Ok(c) => c.into_raw() as *const c_char,
            Err(_) => std::ptr::null(),
        },
        None => std::ptr::null(),
    })
    .unwrap_or_else(std::ptr::null)
}

/// Free a string returned by shorebird_active_path() or
/// shorebird_active_patch_number().
///
/// # Safety
/// `s` must be a pointer returned by one of those functions,
/// or NULL (in which case this is a no-op).
#[no_mangle]
pub extern "C" fn shorebird_free_string(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(s);
    }
}

// =============================================================================
// Internal helpers
// =============================================================================

/// Find the patch with the lexicographically largest directory name.
/// Returns the path to the **patch directory** `<patches_dir>/<max>/`
/// if it contains either `patch.bin` (bsdiff) or `libapp.so` (full file).
fn find_latest_patch(patches_dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(patches_dir).ok()?;
    let mut best: Option<(String, PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        // A patch directory must contain either patch.bin (bsdiff)
        // or libapp.so (full file).
        let has_bsdiff = path.join("patch.bin").exists();
        let has_full = path.join("libapp.so").exists();
        if !has_bsdiff && !has_full {
            debug!("Skipping {}: no patch.bin or libapp.so", name);
            continue;
        }
        match &best {
            Some((n, _)) if n >= &name => {}
            _ => best = Some((name, path)),
        }
    }

    best.map(|(_, p)| p)
}

/// Apply bsdiff patch to base file, write result to output.
fn apply_bsdiff(base: &Path, patch: &Path, output: &Path) -> Result<(), String> {
    if !base.exists() {
        return Err(format!("Base libapp.so not found: {}", base.display()));
    }
    if !patch.exists() {
        return Err(format!("Patch file not found: {}", patch.display()));
    }

    let base_data = fs::read(base).map_err(|e| format!("read base: {}", e))?;
    let patch_data = fs::read(patch).map_err(|e| format!("read patch: {}", e))?;

    info!(
        "Applying bsdiff: base={} bytes, patch={} bytes",
        base_data.len(),
        patch_data.len()
    );

    let result = {
        let patcher = Bspatch::new(&patch_data)
            .map_err(|e| format!("bsdiff parse: {}", e))?;
        let mut out = Vec::with_capacity(patcher.hint_target_size() as usize);
        patcher
            .apply(&base_data, std::io::Cursor::new(&mut out))
            .map_err(|e| format!("bsdiff apply: {}", e))?;
        out
    };

    info!("Patched size: {} bytes", result.len());

    // Ensure parent dir exists
    if let Some(parent) = output.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
        } else if !parent.is_dir() {
            return Err(format!(
                "parent path exists but is not a directory: {}",
                parent.display()
            ));
        }
    }

    fs::write(output, &result).map_err(|e| format!("write output: {}", e))?;

    Ok(())
}

/// Atomic rename: try rename first, fallback to copy+remove on platforms
/// where rename-overwrite isn't supported (very old Windows).
fn atomic_rename(src: &Path, dst: &Path) -> Result<(), String> {
    // On Unix (Android), rename is atomic and overwrites.
    // On Windows, std::fs::rename also overwrites since Rust 1.5+.
    fs::rename(src, dst).map_err(|e| format!("rename: {}", e))
}

/// Verify a file is a valid ELF shared library.
fn verify_elf(path: &Path) -> Result<u64, String> {
    let bytes = fs::read(path).map_err(|e| format!("read: {}", e))?;
    if bytes.len() < 4 {
        return Err("file too small".to_string());
    }
    // ELF magic: 0x7f 'E' 'L' 'F'
    if bytes[0] != 0x7f || bytes[1] != b'E' || bytes[2] != b'L' || bytes[3] != b'F' {
        return Err("not an ELF file (bad magic)".to_string());
    }
    Ok(bytes.len() as u64)
}

/// Find the bundled libapp.so. Tries common Android locations where
/// the system might place extracted native libraries.
fn find_libapp_base(libapp_dir: &Path) -> Option<PathBuf> {
    debug!("Looking for libapp.so under: {}", libapp_dir.display());
    // Direct
    let direct = libapp_dir.join("libapp.so");
    if direct.exists() {
        return Some(direct);
    }
    // Scan subdirectories (arm64, arm64-v8a, etc.)
    if let Ok(entries) = fs::read_dir(libapp_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let p = path.join("libapp.so");
                if p.exists() {
                    return Some(p);
                }
            }
        }
    } else {
        warn!("Cannot read_dir {} (might be sandboxed)", libapp_dir.display());
    }
    // Maybe the system stripped the ABI suffix from the dir name. Try
    // appending common ABI names to libapp_dir.
    for abi in &["arm64-v8a", "arm64", "x86_64", "x86", "armeabi-v7a"] {
        let p = libapp_dir.join(abi).join("libapp.so");
        if p.exists() {
            return Some(p);
        }
    }
    // Try parent dir + ABI subdir
    if let Some(parent) = libapp_dir.parent() {
        for abi in &["arm64-v8a", "arm64", "x86_64", "x86", "armeabi-v7a"] {
            let p = parent.join(abi).join("libapp.so");
            if p.exists() {
                return Some(p);
            }
        }
    }
    // On modern Android, the libapp.so might be in the APK itself
    // (loaded via mmap). In that case there's no extracted file to read.
    // We can find the APK from the package manager, but we don't have
    // access to the package manager from the updater. Log for debugging.
    warn!(
        "libapp.so not found at any common location. On modern Android, \
         the file may be embedded in the APK and not extracted to disk."
    );
    None
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use tempfile::TempDir;

    /// End-to-end: create base, create modified, generate bsdiff,
    /// init updater, apply, verify.
    #[test]
    fn test_full_patch_cycle() {
        let _ = env_logger::builder().is_test(true).try_init();

        let tmp = TempDir::new().unwrap();
        let cache_dir = tmp.path().to_path_buf();
        let libapp_dir = tmp.path().join("lib");
        fs::create_dir_all(&libapp_dir).unwrap();

        // Create "base" libapp.so (1 MB pseudo-random)
        let base_data: Vec<u8> = (0..1024u32 * 1024u32)
            .map(|i| (i.wrapping_mul(31) ^ 0xa5) as u8)
            .collect();
        let base_path = libapp_dir.join("libapp.so");
        fs::write(&base_path, &base_data).unwrap();

        // Create "modified" version (small change in the middle)
        let mut target_data = base_data.clone();
        for i in (500_000..500_100).step_by(1) {
            target_data[i] ^= 0xff;
        }
        // Also append some new bytes
        target_data.extend_from_slice(b"NEW SECTION ADDED AT END");

        // Generate bsdiff (qbsdiff 1.4 API)
        let mut patch_bytes = Vec::new();
        Bsdiff::new(&base_data, &target_data)
            .compare(std::io::Cursor::new(&mut patch_bytes))
            .expect("bsdiff encode");
        info!(
            "Generated bsdiff: {} bytes ({}% of target)",
            patch_bytes.len(),
            patch_bytes.len() * 100 / target_data.len()
        );

        // Save patch in expected location: <cache>/patches/1/patch.bin
        let patch_dir = cache_dir.join("patches").join("1");
        fs::create_dir_all(&patch_dir).unwrap();
        fs::write(patch_dir.join("patch.bin"), &patch_bytes).unwrap();

        // Init
        let v = CString::new("1.0.0+1").unwrap();
        let c = CString::new(cache_dir.to_str().unwrap()).unwrap();
        let l = CString::new(libapp_dir.to_str().unwrap()).unwrap();
        let rc = shorebird_init(v.as_ptr(), c.as_ptr(), l.as_ptr());
        assert_eq!(rc, 0, "shorebird_init failed");

        // Update
        let rc = shorebird_update();
        assert_eq!(rc, 0, "shorebird_update failed");

        // Get active path
        let p = shorebird_active_path();
        assert!(!p.is_null(), "active_path returned NULL");
        let path_str = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_string();
        shorebird_free_string(p as *mut c_char);

        info!("Active path: {}", path_str);

        // Verify patched file matches target
        let patched = fs::read(&path_str).unwrap();
        assert_eq!(patched.len(), target_data.len(), "size mismatch");
        assert_eq!(patched, target_data, "patched content differs from target");

        // Get patch number
        let pn = shorebird_active_patch_number();
        assert!(!pn.is_null(), "active_patch_number returned NULL");
        let pn_str = unsafe { CStr::from_ptr(pn) }.to_str().unwrap();
        assert_eq!(pn_str, "1");
        shorebird_free_string(pn as *mut c_char);
    }

    /// No patches directory → should succeed with no patch
    #[test]
    fn test_no_patches_dir() {
        let _ = env_logger::builder().is_test(true).try_init();

        let tmp = TempDir::new().unwrap();
        let cache_dir = tmp.path().to_path_buf();
        let libapp_dir = tmp.path().join("lib");
        fs::create_dir_all(&libapp_dir).unwrap();
        fs::write(libapp_dir.join("libapp.so"), b"bundled").unwrap();

        let v = CString::new("1.0.0+1").unwrap();
        let c = CString::new(cache_dir.to_str().unwrap()).unwrap();
        let l = CString::new(libapp_dir.to_str().unwrap()).unwrap();
        assert_eq!(shorebird_init(v.as_ptr(), c.as_ptr(), l.as_ptr()), 0);
        assert_eq!(shorebird_update(), 0);

        // Active path should still be bundled
        let p = shorebird_active_path();
        assert!(!p.is_null());
        let s = unsafe { CStr::from_ptr(p) }.to_str().unwrap();
        assert!(s.ends_with("libapp.so"), "expected libapp.so, got: {}", s);
        // Should NOT be in cache/patches (no patch applied)
        assert!(!s.contains("patches"), "should be bundled, got: {}", s);
        shorebird_free_string(p as *mut c_char);

        // No active patch number
        let pn = shorebird_active_patch_number();
        assert!(pn.is_null());
    }

    /// Multiple patches → should pick the lexicographically largest
    #[test]
    fn test_pick_latest_of_multiple() {
        let _ = env_logger::builder().is_test(true).try_init();

        let tmp = TempDir::new().unwrap();
        let cache_dir = tmp.path().to_path_buf();
        let libapp_dir = tmp.path().join("lib");
        fs::create_dir_all(&libapp_dir).unwrap();

        let base: Vec<u8> = (0..4096).map(|i| i as u8).collect();
        fs::write(libapp_dir.join("libapp.so"), &base).unwrap();

        // Two patches: "1" and "2"
        for patch_num in &["1", "2"] {
            let target = format!("target-{}", patch_num);
            let target_bytes = target.as_bytes();

            let mut patch_bytes = Vec::new();
            Bsdiff::new(&base, target_bytes)
                .compare(std::io::Cursor::new(&mut patch_bytes))
                .expect("bsdiff encode");

            let dir = cache_dir.join("patches").join(patch_num);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("patch.bin"), &patch_bytes).unwrap();
        }

        let v = CString::new("1.0.0+1").unwrap();
        let c = CString::new(cache_dir.to_str().unwrap()).unwrap();
        let l = CString::new(libapp_dir.to_str().unwrap()).unwrap();
        shorebird_init(v.as_ptr(), c.as_ptr(), l.as_ptr());
        shorebird_update();

        let pn = shorebird_active_patch_number();
        assert!(!pn.is_null());
        let s = unsafe { CStr::from_ptr(pn) }.to_str().unwrap();
        assert_eq!(s, "2", "should pick lexicographically largest");
        shorebird_free_string(pn as *mut c_char);
    }

    /// Bad bsdiff patch → should fail cleanly
    #[test]
    fn test_corrupt_patch_fails() {
        let _ = env_logger::builder().is_test(true).try_init();

        let tmp = TempDir::new().unwrap();
        let cache_dir = tmp.path().to_path_buf();
        let libapp_dir = tmp.path().join("lib");
        fs::create_dir_all(&libapp_dir).unwrap();
        fs::write(libapp_dir.join("libapp.so"), b"some bundled content").unwrap();

        // Create a patch dir with garbage
        let dir = cache_dir.join("patches").join("bad");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("patch.bin"), b"NOT A VALID BSDIFF").unwrap();

        let v = CString::new("1.0.0+1").unwrap();
        let c = CString::new(cache_dir.to_str().unwrap()).unwrap();
        let l = CString::new(libapp_dir.to_str().unwrap()).unwrap();
        shorebird_init(v.as_ptr(), c.as_ptr(), l.as_ptr());

        let rc = shorebird_update();
        assert_ne!(rc, 0, "should fail on corrupt patch");
    }
}
