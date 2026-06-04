//! pultflut-bspatch — apply a bsdiff patch to produce a new file.
//!
//! Usage: pultflut-bspatch <source> <patch> <output>
//!   source  = base libapp.so
//!   patch   = bsdiff binary
//!   output  = new libapp.so
//!
//! Useful for testing patches on the dev machine before pushing to device.

use std::env;
use std::fs;

use qbsdiff::Bspatch;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: pultflut-bspatch <source> <patch> <output>");
        eprintln!();
        eprintln!("Example:");
        eprintln!("  pultflut-bspatch libapp_v1.so patch_2.bin libapp_v2_reconstructed.so");
        std::process::exit(1);
    }

    let src_path = &args[1];
    let patch_path = &args[2];
    let out_path = &args[3];

    let source = match fs::read(src_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error reading source: {}", e);
            std::process::exit(1);
        }
    };
    let patch = match fs::read(patch_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error reading patch: {}", e);
            std::process::exit(1);
        }
    };

    eprintln!("Parsing patch ({} bytes)...", patch.len());
    let patcher = match Bspatch::new(&patch) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to parse patch: {}", e);
            std::process::exit(1);
        }
    };

    eprintln!("Applying patch...");
    let mut output = Vec::with_capacity(patcher.hint_target_size() as usize);
    if let Err(e) = patcher.apply(&source, std::io::Cursor::new(&mut output)) {
        eprintln!("Failed to apply patch: {}", e);
        std::process::exit(1);
    }

    if let Err(e) = fs::write(out_path, &output) {
        eprintln!("Failed to write output: {}", e);
        std::process::exit(1);
    }

    eprintln!("✓ Output written: {} ({} bytes)", out_path, output.len());
}
