use std::path::{Path, PathBuf};
use std::process::Command;

const PROBE_RUST_GIT: &str = "https://github.com/Beneficial-AI-Foundation/probe-rust.git";
const PROBE_LEAN_GIT: &str = "https://github.com/Beneficial-AI-Foundation/probe-lean.git";

/// Run `probe-rust extract` on a project and return the path to the generated JSON.
pub fn run_probe_rust_extract(project: &Path) -> Result<PathBuf, String> {
    let bin = find_or_install_probe_rust()?;
    let output = tempfile("probe_rust", ".json");

    println!("Running probe-rust extract on {}...", project.display());
    let status = Command::new(&bin)
        .args([
            "extract",
            project.to_str().unwrap_or("."),
            "-o",
            output.to_str().unwrap_or("."),
            "--auto-install",
            "--with-charon",
        ])
        .status()
        .map_err(|e| format!("Failed to run probe-rust: {e}"))?;

    if !status.success() {
        return Err(format!(
            "probe-rust extract exited with status {}",
            status.code().unwrap_or(-1)
        ));
    }

    if !output.exists() {
        return Err(format!(
            "probe-rust extract completed but {} was not created",
            output.display()
        ));
    }

    println!("  ✓ Rust atoms: {}", output.display());
    Ok(output)
}

/// Run `probe-lean extract` on a project and return the path to the generated JSON.
pub fn run_probe_lean_extract(project: &Path) -> Result<PathBuf, String> {
    let bin = find_or_install_probe_lean()?;
    let output = tempfile("probe_lean", ".json");

    println!("Running probe-lean extract on {}...", project.display());
    let status = Command::new(&bin)
        .args([
            "extract",
            project.to_str().unwrap_or("."),
            "-o",
            output.to_str().unwrap_or("."),
        ])
        .status()
        .map_err(|e| format!("Failed to run probe-lean: {e}"))?;

    if !status.success() {
        return Err(format!(
            "probe-lean extract exited with status {}",
            status.code().unwrap_or(-1)
        ));
    }

    if !output.exists() {
        return Err(format!(
            "probe-lean extract completed but {} was not created",
            output.display()
        ));
    }

    println!("  ✓ Lean atoms: {}", output.display());
    Ok(output)
}

fn find_or_install_probe_rust() -> Result<PathBuf, String> {
    if let Some(p) = find_on_path("probe-rust") {
        return Ok(p);
    }

    let cargo_bin = home_dir()?.join(".cargo/bin/probe-rust");
    if cargo_bin.exists() {
        return Ok(cargo_bin);
    }

    println!("probe-rust not found, installing via cargo...");
    let status = Command::new("cargo")
        .args(["install", "--git", PROBE_RUST_GIT])
        .status()
        .map_err(|e| format!("Failed to run cargo install: {e}"))?;

    if !status.success() {
        return Err(
            "cargo install probe-rust failed. Please install manually:\n  \
             cargo install --git https://github.com/Beneficial-AI-Foundation/probe-rust.git"
                .to_string(),
        );
    }

    if cargo_bin.exists() {
        Ok(cargo_bin)
    } else {
        Err("cargo install succeeded but probe-rust binary not found in ~/.cargo/bin/".to_string())
    }
}

fn find_or_install_probe_lean() -> Result<PathBuf, String> {
    if let Some(p) = find_on_path("probe-lean") {
        return Ok(p);
    }

    let local_bin = home_dir()?.join(".local/bin/probe-lean");
    if local_bin.exists() {
        return Ok(local_bin);
    }

    println!("probe-lean not found, installing from source...");

    let build_dir = std::env::temp_dir().join("probe-lean-build");
    if build_dir.exists() {
        std::fs::remove_dir_all(&build_dir)
            .map_err(|e| format!("Failed to clean build dir: {e}"))?;
    }

    let status = Command::new("git")
        .args(["clone", "--depth", "1", PROBE_LEAN_GIT])
        .arg(&build_dir)
        .status()
        .map_err(|e| format!("Failed to clone probe-lean: {e}"))?;

    if !status.success() {
        return Err("git clone probe-lean failed".to_string());
    }

    let status = Command::new("lake")
        .arg("build")
        .current_dir(&build_dir)
        .status()
        .map_err(|e| format!("Failed to build probe-lean with lake: {e}"))?;

    if !status.success() {
        return Err(
            "lake build failed. Make sure elan/lean4 and lake are installed.\n  \
             See: https://github.com/leanprover/elan"
                .to_string(),
        );
    }

    let built_bin = build_dir.join(".lake/build/bin/probe-lean");
    if !built_bin.exists() {
        return Err("lake build succeeded but .lake/build/bin/probe-lean not found".to_string());
    }

    let dest_dir = home_dir()?.join(".local/bin");
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create ~/.local/bin: {e}"))?;

    std::fs::copy(&built_bin, &local_bin)
        .map_err(|e| format!("Failed to copy probe-lean to ~/.local/bin: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&local_bin, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to set executable permission: {e}"))?;
    }

    println!("  ✓ Installed probe-lean to {}", local_bin.display());
    Ok(local_bin)
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    which::which(name).ok()
}

fn home_dir() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "Could not determine home directory".to_string())
}

fn tempfile(prefix: &str, suffix: &str) -> PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    std::env::temp_dir().join(format!("{prefix}_{ts}{suffix}"))
}
