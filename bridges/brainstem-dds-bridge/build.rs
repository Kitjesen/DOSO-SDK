use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_cyclonedds)");
    println!("cargo:rerun-if-env-changed=CYCLONEDDS_ROOT");
    println!("cargo:rerun-if-changed=csrc/bridge_messages.c");
    println!("cargo:rerun-if-changed=csrc/bridge_messages.h");
    println!("cargo:rerun-if-changed=csrc/bridge_ddsc.c");
    println!("cargo:rerun-if-changed=csrc/bridge_ddsc.h");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let target = env::var("TARGET").unwrap_or_default();
    let host = env::var("HOST").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    let mut candidates = Vec::new();
    if let Ok(root) = env::var("CYCLONEDDS_ROOT") {
        let trimmed = root.trim();
        if trimmed.eq_ignore_ascii_case("off")
            || trimmed.eq_ignore_ascii_case("none")
            || trimmed.eq_ignore_ascii_case("disable")
            || trimmed.eq_ignore_ascii_case("false")
        {
            println!(
                "cargo:warning=CycloneDDS explicitly disabled via CYCLONEDDS_ROOT={trimmed}; building standalone Board-A Base Gateway"
            );
            return;
        }
        if !trimmed.is_empty() {
            candidates.push(PathBuf::from(trimmed));
        }
    }
    if target_os == "windows" {
        candidates.push(
            manifest_dir
                .join("../../../lingtu/third_party/integration/cyclonedds-windows-20260817-d/sdk"),
        );
    } else if target == host {
        candidates.push(PathBuf::from("/usr"));
        candidates.push(PathBuf::from("/usr/local"));
    }

    let sdk_root = candidates
        .into_iter()
        .find(|root| root.join("include/dds/dds.h").is_file());

    let Some(sdk_root) = sdk_root else {
        println!(
            "cargo:warning=CycloneDDS SDK not found; building brainstem-dds-bridge without live ddsc backend"
        );
        return;
    };

    let include_dir = sdk_root.join("include");
    let lib_dir = sdk_root.join("lib");
    let csrc_dir = manifest_dir.join("csrc");

    cc::Build::new()
        .include(&csrc_dir)
        .include(&include_dir)
        .file(csrc_dir.join("bridge_messages.c"))
        .file(csrc_dir.join("bridge_ddsc.c"))
        .warnings(false)
        .compile("brainstem_bridge_ddsc");

    if lib_dir.is_dir() {
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
    }
    println!("cargo:rustc-link-lib=ddsc");
    println!("cargo:rustc-cfg=has_cyclonedds");
}
