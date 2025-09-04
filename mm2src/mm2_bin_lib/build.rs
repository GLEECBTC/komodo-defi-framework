use chrono::Utc;
use regex::Regex;
use std::{env, process::Command};

fn crate_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn version_tag() -> Result<String, String> {
    if let Ok(tag) = env::var("KDF_BUILD_TAG") {
        return Ok(tag);
    }

    let output = Command::new("git")
        .current_dir("../../")
        .args(["log", "--pretty=format:%h", "-n1"])
        .output()
        .map_err(|e| format!("Failed to run git command: {e}\nSet `KDF_BUILD_TAG` manually instead.",))?;

    let commit_hash = String::from_utf8(output.stdout)
        .map_err(|e| format!("Invalid UTF-8 sequence: {e}"))?
        .trim()
        .to_string();

    if !Regex::new(r"^\w+$")
        .expect("Failed to compile regex")
        .is_match(&commit_hash)
    {
        return Err(format!("Invalid tag: {commit_hash}"));
    }

    Ok(commit_hash)
}

fn version() -> Result<String, String> {
    version_tag().map(|tag| format!("{}_{}", crate_version(), tag))
}

fn build_datetime() -> String {
    Utc::now().to_rfc3339()
}

fn set_build_variables() -> Result<(), String> {
    println!("cargo:rustc-env=KDF_VERSION={}", version()?);
    println!("cargo:rustc-env=KDF_DATETIME={}", build_datetime());
    Ok(())
}

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
    println!("cargo:rerun-if-env-changed=KDF_BUILD_TAG");
    println!("cargo::rerun-if-changed=../../.git/HEAD");

    set_build_variables().expect("Failed to set build variables");
}
