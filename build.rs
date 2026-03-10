use std::process::Command;

fn main() {
    // Build frontend if it exists
    if cfg!(feature = "webui") && std::path::Path::new("frontend").exists() {
        println!("cargo:info=Building frontend...");

        let command = command_fallback(0);
        let status = Command::new(command)
            .args(&["run", "build"])
            .current_dir("frontend")
            .status()
            .expect("Failed to execute frontend build command");

        if !status.success() {
            println!("cargo:warning=Frontend build failed. Attempting to install dependencies...");

            let install_status = Command::new(command)
                .args(&["install"])
                .current_dir("frontend")
                .status()
                .expect("Failed to execute frontend install command");

            if !install_status.success() {
                panic!(
                    "Frontend install failed with status: {}. {}",
                    install_status,
                    "Make sure the install command is correct and that all dependencies are available."
                );
            }

            println!("cargo:info=Dependencies installed. Retrying build...");

            let retry_status = Command::new(command)
                .args(&["run", "build"])
                .current_dir("frontend")
                .status()
                .expect("Failed to execute frontend build command after installing dependencies");

            if !retry_status.success() {
                panic!(
                    "Frontend build failed again with status: {}. {}",
                    retry_status,
                    "Ensure the build command is correct and all dependencies are properly installed."
                );
            }
        }

        println!("cargo:info=Frontend build completed successfully.");
    }

    // Rebuild if frontend source changes
    println!("cargo:rerun-if-changed=frontend/src");
    println!("cargo:rerun-if-changed=frontend/package.json");
}

const CANDIDATE_COMMANDS: &[&'static str] = &["pnpm", "yarn", "npm"];

fn command_fallback(ix: usize) -> &'static str {
    let status = Command::new(CANDIDATE_COMMANDS[ix])
        .args(&["--version"])
        .status();
    if let Ok(status) = status {
        println!(
            "cargo:info=Command '{}' is available with version: {}. {}",
            CANDIDATE_COMMANDS[ix], status, "Running frontend build with this command..."
        );
        CANDIDATE_COMMANDS[ix]
    } else if ix + 1 < CANDIDATE_COMMANDS.len() {
        command_fallback(ix + 1)
    } else {
        panic!(
            "Failed to execute any of the candidate commands: {:?}. {}",
            CANDIDATE_COMMANDS,
            "Make sure at least one of them is installed and available in PATH."
        );
    }
}
