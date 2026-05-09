use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    // Pass the target triple as an env var so ptp.rs can find the binary
    let target = env::var("TARGET").unwrap_or_else(|_| "aarch64-apple-darwin".to_string());
    println!("cargo:rustc-env=TARGET_TRIPLE={}", target);

    // Compile Swift PTP bridge binary
    let swift_src = "swift/ptp_bridge.swift";
    let out_binary = format!("binaries/ptp-bridge-{}", target);

    println!("cargo:rerun-if-changed={}", swift_src);

    // Only compile if the source is newer than the binary
    let should_compile = if Path::new(&out_binary).exists() {
        let src_modified = std::fs::metadata(swift_src)
            .and_then(|m| m.modified())
            .ok();
        let bin_modified = std::fs::metadata(&out_binary)
            .and_then(|m| m.modified())
            .ok();

        match (src_modified, bin_modified) {
            (Some(src), Some(bin)) => src > bin,
            _ => true,
        }
    } else {
        true
    };

    if should_compile {
        // Ensure binaries directory exists
        std::fs::create_dir_all("binaries").expect("Failed to create binaries directory");

        let status = Command::new("swiftc")
            .args([
                "-O",
                "-framework",
                "ImageCaptureCore",
                "-framework",
                "CoreGraphics",
                "-framework",
                "ImageIO",
                "-o",
                &out_binary,
                swift_src,
            ])
            .status()
            .expect("Failed to run swiftc. Is Xcode command-line tools installed?");

        if !status.success() {
            panic!("Swift PTP bridge compilation failed");
        }

        println!("cargo:warning=Compiled PTP bridge binary: {}", out_binary);
    }

    tauri_build::build()
}
