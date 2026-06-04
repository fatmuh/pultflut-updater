//! JNI shim - exposes our C API to Java/Kotlin.
//!
//! This module is only compiled for Android targets. It provides
//! JNI-exported functions named `Java_dev_pultflut_PultflutNative_*`
//! that wrap our core API in `lib.rs`.
//!
//! Kotlin side (PultflutNative.kt):
//! ```kotlin
//! object PultflutNative {
//!     init { System.loadLibrary("pultflut_updater") }
//!     external fun shorebirdInit(version: String, cacheDir: String, libappDir: String): Int
//!     external fun shorebirdUpdate(): Int
//!     external fun shorebirdActivePath(): String?
//!     external fun shorebirdActivePatchNumber(): String?
//!     external fun shorebirdFreeString(s: String?)
//! }
//! ```

#[cfg(target_os = "android")]
mod android_impl {
    use jni::JNIEnv;
    use jni::objects::{JClass, JString};
    use jni::sys::{jint, jstring};
    use std::ffi::CString;
    use std::os::raw::c_char;
    use std::ptr;

    use crate::{shorebird_active_patch_number, shorebird_active_path, shorebird_free_string, shorebird_init, shorebird_update};

    /// Convert a `*const c_char` to a Kotlin String, freeing the C string.
    unsafe fn cstr_to_kstring(env: &JNIEnv, ptr: *const c_char) -> jstring {
        if ptr.is_null() {
            return ptr::null_mut();
        }
        // Read the C string
        let cstr = unsafe { std::ffi::CStr::from_ptr(ptr) };
        let s = match cstr.to_str() {
            Ok(s) => s.to_owned(),
            Err(_) => {
                shorebird_free_string(ptr as *mut c_char);
                return ptr::null_mut();
            }
        };
        // Free the C string (we own it)
        shorebird_free_string(ptr as *mut c_char);
        // Return Java String
        match env.new_string(&s) {
            Ok(js) => js.into_raw(),
            Err(_) => ptr::null_mut(),
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_dev_pultflut_PultflutNative_nativeShorebirdInit(
        mut env: JNIEnv,
        _class: JClass,
        version: JString,
        cache_dir: JString,
        libapp_dir: JString,
    ) -> jint {
        let v_str = match env.get_string(&version) {
            Ok(s) => s,
            Err(_) => return -1,
        };
        let c_str = match env.get_string(&cache_dir) {
            Ok(s) => s,
            Err(_) => return -1,
        };
        let l_str = match env.get_string(&libapp_dir) {
            Ok(s) => s,
            Err(_) => return -1,
        };

        let v: String = v_str.into();
        let c: String = c_str.into();
        let l: String = l_str.into();

        let c_v = match CString::new(v) {
            Ok(s) => s,
            Err(_) => return -1,
        };
        let c_c = match CString::new(c) {
            Ok(s) => s,
            Err(_) => return -1,
        };
        let c_l = match CString::new(l) {
            Ok(s) => s,
            Err(_) => return -1,
        };

        shorebird_init(c_v.as_ptr(), c_c.as_ptr(), c_l.as_ptr())
    }

    #[no_mangle]
    pub extern "system" fn Java_dev_pultflut_PultflutNative_nativeShorebirdUpdate(
        _env: JNIEnv,
        _class: JClass,
    ) -> jint {
        shorebird_update()
    }

    #[no_mangle]
    pub extern "system" fn Java_dev_pultflut_PultflutNative_nativeShorebirdActivePath(
        env: JNIEnv,
        _class: JClass,
    ) -> jstring {
        let p: *const c_char = shorebird_active_path();
        unsafe { cstr_to_kstring(&env, p) }
    }

    #[no_mangle]
    pub extern "system" fn Java_dev_pultflut_PultflutNative_nativeShorebirdActivePatchNumber(
        env: JNIEnv,
        _class: JClass,
    ) -> jstring {
        let p: *const c_char = shorebird_active_patch_number();
        unsafe { cstr_to_kstring(&env, p) }
    }
}

#[cfg(target_os = "android")]
pub use android_impl::*;
