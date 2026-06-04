# Pultflut Updater

Native updater library (`libupdater.so`) untuk Pultflut. Handles runtime patching of `libapp.so` di Android apps pakai bsdiff.

## Cara kerja

```
[APK boot]
  └─ PultflutApplication.attachBaseContext()
       └─ Java: shorebird_init()           ← set cache dir
       └─ Java: shorebird_update()         ← apply bsdiff patch
       └─ Java: shorebird_active_path()    ← get path to patched libapp.so
       └─ Java: set application_library_path
       └─ Engine loads patched libapp.so
```

## Build

```bash
# Android
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 build --release
# Output: target/aarch64-linux-android/release/libpultflut_updater.so
```

## API

```c
// Init dengan parameter aplikasi
int shorebird_init(const char* release_version,
                   const char* cache_dir,
                   const char* libapp_dir);

// Apply patch + check active
int shorebird_update(void);

// Get path ke patched libapp.so
const char* shorebird_active_path(void);
const char* shorebird_active_patch_number(void);

// Free string yang di-return oleh active_path/active_patch_number
void shorebird_free_string(char* s);
```

## Status

🚧 Phase 1 (PoC) — bsdiff apply to dummy file
