//! pultflut-bsdiff — generate a bsdiff patch between two files.
//!
//! Usage: pultflut-bsdiff <source> <target> <output>
//!   source  = bundled libapp.so (old version)
//!   target  = new libapp.so (new version)
//!   output  = .bin patch file
//!
//! This is the dev-side tool. The Android device uses `shorebird_update()`
//! (in lib.rs) to apply the patch at runtime.

use std::env;
use std::fs;
use std::io::Cursor;

use qbsdiff::Bsdiff;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: pultflut-bsdiff <source> <target> <output>");
        eprintln!();
        eprintln!("Example:");
        eprintln!("  pultflut-bsdiff libapp_v1.so libapp_v2.so patch_2.bin");
        std::process::exit(1);
    }

    let src_path = &args[1];
    let tgt_path = &args[2];
    let out_path = &args[3];

    eprintln!("Reading source: {}", src_path);
    let source = match fs::read(src_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error reading source: {}", e);
            std::process::exit(1);
        }
    };

    eprintln!("Reading target: {}", tgt_path);
    let target = match fs::read(tgt_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error reading target: {}", e);
            std::process::exit(1);
        }
    };

    eprintln!(
        "Encoding bsdiff: source={} bytes, target={} bytes",
        source.len(),
        target.len()
    );

    let mut patch = Vec::new();
    if let Err(e) = Bsdiff::new(&source, &target).compare(Cursor::new(&mut patch)) {
        eprintln!("bsdiff encode failed: {}", e);
        std::process::exit(1);
    }

    if let Err(e) = fs::write(out_path, &patch) {
        eprintln!("Failed to write output: {}", e);
        std::process::exit(1);
    }

    eprintln!(
        "✓ Patch written: {} ({}% of target, {:.2}x compression)",
        out_path,
        patch.len() * 100 / target.len().max(1),
        target.len() as f64 / patch.len().max(1) as f64
    );
}
