// The script here will translate some of the C headers necessary for the gradual Rust port into the corresponding Rust files.
// Going to take the *whitelisting* approach, converting just the necessary definitions, in order to keep the builds fast.

// The script is experimentally formatted with `rustfmt`. Probably not going to use `rustfmt` for the rest of the project though.

// Bindgen requirements: https://rust-lang.github.io/rust-bindgen/requirements.html
//              Windows: https://github.com/rust-lang-nursery/rustup.rs/issues/1003#issuecomment-289825927
// On build.rs: https://doc.rust-lang.org/cargo/reference/build-scripts.html

#![allow(uncommon_codepoints)]

use gstuff::{last_modified_sec, slurp};
use std::env::var;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Ongoing (RLS) builds might interfere with a precise time comparison.
const SLIDE: f64 = 60.;

/// Loads the `path`, runs `update` on it and saves back the result if it differs.
fn _in_place(path: &dyn AsRef<Path>, update: &mut dyn FnMut(Vec<u8>) -> Vec<u8>) {
    let path: &Path = path.as_ref();
    if !path.is_file() {
        return;
    }
    let dir = path.parent().unwrap();
    let name = path.file_name().unwrap().to_str().unwrap();
    let bulk = slurp(&path);
    if bulk.is_empty() {
        return;
    }
    let updated = update(bulk.clone());
    if bulk != updated {
        let tmp = dir.join(format!("{name}.tmp"));
        {
            let mut file = fs::File::create(&tmp).unwrap();
            file.write_all(&updated).unwrap();
        }
        fs::rename(tmp, path).unwrap()
    }
}

/// The target build architecture.
///
/// # Note
///
/// Please expand this enum if it is necessary.
enum TargetArch {
    Wasm32,
    Other,
}

impl TargetArch {
    fn detect() -> Option<TargetArch> {
        match var("CARGO_CFG_TARGET_ARCH") {
            Ok(arch) => Some(TargetArch::from(arch)),
            Err(e) => {
                eprintln!("Error on get CARGO_CFG_TARGET_ARCH env: {e}");
                None
            },
        }
    }
}

impl From<String> for TargetArch {
    fn from(arch: String) -> Self {
        match arch.as_str() {
            "wasm32" => TargetArch::Wasm32,
            _ => TargetArch::Other,
        }
    }
}

/// Build helper C code.
///
/// I think "git clone ... && cargo build" should be enough to start hacking on the Rust code.
///
/// For now we're building the Structured Exception Handling code here,
/// but in the future we might subsume the rest of the C build under build.rs.
fn build_c_code() {
    if let Some(TargetArch::Wasm32) = TargetArch::detect() {
        return;
    }

    let target_os = var("CARGO_CFG_TARGET_FAMILY").expect("!CARGO_CFG_TARGET_FAMILY");
    if target_os == "windows" {
        // Link in the Windows-specific crash handling code.
        let lm_seh = last_modified_sec(&"seh.c").expect("Can't stat seh.c");
        let out_dir = var("OUT_DIR").expect("!OUT_DIR");
        let lib_path = Path::new(&out_dir).join("libseh.a");
        let lm_lib = last_modified_sec(&lib_path).unwrap_or(0.);
        if lm_seh >= lm_lib - SLIDE {
            cc::Build::new().file("seh.c").warnings(true).compile("seh");
        }
        println!("cargo:rustc-link-lib=static=seh");
        println!("cargo:rustc-link-search=native={out_dir}");
    }
}

fn main() {
    // NB: `rerun-if-changed` will ALWAYS invoke the build.rs if the target does not exists.
    // cf. https://github.com/rust-lang/cargo/issues/4514#issuecomment-330976605
    //     https://github.com/rust-lang/cargo/issues/4213#issuecomment-310697337
    // `RUST_LOG=cargo::core::compiler::fingerprint cargo build` shows the fingerprit files used.
    println!("cargo:rerun-if-changed=seh.c");
    build_c_code();
}
