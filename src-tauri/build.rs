use std::env;
use std::process::Command;

fn main() {
    // Pass the target triple as an env var so ptp.rs can find the binary
    let target = env::var("TARGET").unwrap_or_else(|_| "aarch64-apple-darwin".to_string());
    println!("cargo:rustc-env=TARGET_TRIPLE={}", target);

    // Compile Swift PTP bridge binary
    let swift_src = "swift/ptp_bridge.swift";
    let out_binary = format!("binaries/ptp-bridge-{}", target);

    println!("cargo:rerun-if-changed={}", swift_src);

    // Always compile the bridge for Apple targets. Git checkouts do not
    // preserve source mtimes, so the previous source-newer-than-binary check
    // could silently bundle a stale checked-in bridge after Swift changes.
    if target.contains("apple-darwin") {
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
