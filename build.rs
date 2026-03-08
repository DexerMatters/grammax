use std::process::Command;

fn main() {
    // Build frontend if it exists
    if std::path::Path::new("frontend").exists() {
        println!("cargo:warning=Building frontend...");

        let status = Command::new("npm")
            .args(&["run", "build"])
            .current_dir("frontend")
            .status()
            .expect("Failed to execute npm");

        if !status.success() {
            panic!("Frontend build failed");
        }
    }

    // Rebuild if frontend source changes
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/package.json");
}
