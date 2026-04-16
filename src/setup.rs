//! `setup` subcommand: install and manage external tool dependencies.
//!
//! Manages probe-rust and charon. After installing probe-rust, delegates to
//! `probe-rust setup` to install its own dependencies (rust-analyzer, scip).
//! probe-lean is version-matched to each target project's `lean-toolchain`
//! and is auto-installed per-project during `extract`, so it is not handled
//! here.

use std::path::PathBuf;
use std::process::Command;

const PROBE_RUST_GIT: &str = "https://github.com/Beneficial-AI-Foundation/probe-rust.git";
const CHARON_REPO: &str = "https://github.com/AeneasVerif/charon.git";

// ---------------------------------------------------------------------------
// Public installation functions
// ---------------------------------------------------------------------------

/// Clone and build charon from source into `~/.probe-rust/tools/`.
///
/// Mirrors probe-rust's `tool_manager::build_charon` so both tools share the
/// same managed binary. Both `charon` and `charon-driver` are installed.
/// Reuses existing source checkout if present.
pub fn install_charon() -> Result<(), String> {
    let tools_dir = home_dir()?.join(".probe-rust/tools");
    std::fs::create_dir_all(&tools_dir)
        .map_err(|e| format!("Failed to create {}: {e}", tools_dir.display()))?;

    let src_dir = tools_dir.join("charon-src");

    if !src_dir.join("charon").join("Cargo.toml").exists() {
        eprintln!("Cloning charon from {CHARON_REPO}...");
        let status = Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                CHARON_REPO,
                &src_dir.to_string_lossy(),
            ])
            .status()
            .map_err(|e| format!("Failed to clone charon: {e}"))?;
        if !status.success() {
            return Err("git clone charon failed".to_string());
        }
    }

    eprintln!("Building charon (this may take a few minutes)...");
    let status = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(src_dir.join("charon"))
        .status()
        .map_err(|e| format!("Failed to build charon: {e}"))?;
    if !status.success() {
        return Err("cargo build --release charon failed".to_string());
    }

    let release_dir = src_dir.join("charon/target/release");
    for binary in ["charon", "charon-driver"] {
        let src = release_dir.join(binary);
        let dst = tools_dir.join(binary);
        std::fs::copy(&src, &dst)
            .map_err(|e| format!("Failed to copy {binary} to {}: {e}", dst.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("Failed to set permissions on {binary}: {e}"))?;
        }
    }

    eprintln!("  ✓ Installed charon to {}", tools_dir.display());
    Ok(())
}

/// Install probe-rust via `cargo install --git`.
pub fn install_probe_rust() -> Result<PathBuf, String> {
    let cargo_bin = home_dir()?.join(".cargo/bin/probe-rust");
    if cargo_bin.exists() {
        return Ok(cargo_bin);
    }

    eprintln!("Installing probe-rust via cargo...");
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

// ---------------------------------------------------------------------------
// Resolution helpers (shared with extract_runner)
// ---------------------------------------------------------------------------

pub fn find_on_path(name: &str) -> Option<PathBuf> {
    which::which(name).ok()
}

pub fn home_dir() -> Result<PathBuf, String> {
    dirs::home_dir().ok_or_else(|| "Could not determine home directory".to_string())
}

/// Resolve probe-rust binary: PATH then `~/.cargo/bin/`.
pub fn resolve_probe_rust() -> Option<PathBuf> {
    if let Some(p) = find_on_path("probe-rust") {
        return Some(p);
    }
    let cargo_bin = home_dir().ok()?.join(".cargo/bin/probe-rust");
    if cargo_bin.exists() {
        Some(cargo_bin)
    } else {
        None
    }
}

/// Resolve charon binary: managed directory then PATH.
pub fn resolve_charon() -> Option<PathBuf> {
    let managed = home_dir().ok()?.join(".probe-rust/tools/charon");
    if managed.exists() {
        return Some(managed);
    }
    find_on_path("charon")
}

/// Ensure the `rust-analyzer` rustup component is installed.
///
/// When `toolchain` is `Some("nightly-2026-03-23")`, targets that specific
/// toolchain; when `None`, targets the default toolchain.
pub fn ensure_rust_analyzer_component(toolchain: Option<&str>) -> Result<(), String> {
    let mut args = vec!["component", "add", "rust-analyzer"];
    if let Some(tc) = toolchain {
        args.push("--toolchain");
        args.push(tc);
    }
    let label = toolchain.unwrap_or("default");
    eprintln!("Ensuring rust-analyzer is installed for {label} toolchain...");

    let output = Command::new("rustup")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to run rustup: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "rustup component add rust-analyzer failed for {label} toolchain: {stderr}"
        ));
    }
    eprintln!("  ✓ rust-analyzer available for {label} toolchain");
    Ok(())
}

/// Delegate to `probe-rust setup` to install probe-rust's own dependencies
/// (rust-analyzer, scip). The `probe_rust_bin` must already be installed.
fn run_probe_rust_setup(probe_rust_bin: &std::path::Path) -> Result<(), String> {
    eprintln!("\nRunning probe-rust setup to install its dependencies...\n");
    let status = Command::new(probe_rust_bin)
        .arg("setup")
        .status()
        .map_err(|e| format!("Failed to run probe-rust setup: {e}"))?;
    if !status.success() {
        return Err("probe-rust setup failed. Run it manually for details:\n  \
                     probe-rust setup"
            .to_string());
    }
    Ok(())
}

/// Resolve probe-lean binary (any version on PATH or in `~/.local/bin/`).
fn resolve_probe_lean() -> Option<PathBuf> {
    if let Some(p) = find_on_path("probe-lean") {
        return Some(p);
    }
    let local_bin = home_dir().ok()?.join(".local/bin/probe-lean");
    if local_bin.exists() {
        Some(local_bin)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Status reporting
// ---------------------------------------------------------------------------

fn status_line(name: &str, location: &Option<PathBuf>, note: &str) {
    match location {
        Some(p) => eprintln!("  {name:<16} {}", p.display()),
        None => eprintln!("  {name:<16} missing{note}"),
    }
}

/// Print a human-readable status table for all managed tools.
pub fn print_status() {
    let tools_dir = home_dir()
        .map(|h| h.join(".probe-rust/tools"))
        .unwrap_or_else(|_| PathBuf::from("<unknown>"));

    eprintln!();
    eprintln!("Managed tools directory: {}", tools_dir.display());
    eprintln!();

    let probe_rust = resolve_probe_rust();
    let charon = resolve_charon();
    let probe_lean = resolve_probe_lean();

    status_line("probe-rust", &probe_rust, "");
    status_line("charon", &charon, "");
    status_line(
        "probe-lean",
        &probe_lean,
        " (installed per-project during extract)",
    );
    eprintln!();

    if let Some(ref pr) = probe_rust {
        eprintln!("probe-rust dependencies (rust-analyzer, scip):");
        let _ = Command::new(pr).args(["setup", "--status"]).status();
    }
}

// ---------------------------------------------------------------------------
// CLI handler
// ---------------------------------------------------------------------------

/// Entry point for the `setup` subcommand.
pub fn cmd_setup(status: bool) {
    if status {
        print_status();
        return;
    }

    eprintln!("Installing external tools for probe-aeneas...\n");

    let mut errors: Vec<String> = Vec::new();

    // probe-rust (install if needed, then delegate to its setup for deps)
    let probe_rust_bin = match resolve_probe_rust() {
        Some(p) => {
            eprintln!("probe-rust: already available at {}", p.display());
            Some(p)
        }
        None => match install_probe_rust() {
            Ok(p) => Some(p),
            Err(e) => {
                errors.push(format!("probe-rust: {e}"));
                None
            }
        },
    };

    // rust-analyzer + scip (delegated to probe-rust setup)
    if let Some(ref bin) = probe_rust_bin {
        if let Err(e) = run_probe_rust_setup(bin) {
            errors.push(e);
        }
    }

    // Ensure rust-analyzer is installed for the default toolchain.
    // probe-rust setup only *checks* for it (warning, not error), so we
    // install the rustup component directly as a fallback.
    if let Err(e) = ensure_rust_analyzer_component(None) {
        errors.push(e);
    }

    // charon
    match resolve_charon() {
        Some(p) => eprintln!("charon: already available at {}", p.display()),
        None => {
            if let Err(e) = install_charon() {
                errors.push(format!("charon: {e}"));
            }
        }
    }

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("Error: {e}");
        }
        eprintln!(
            "\n{} tool(s) failed to install. See errors above.",
            errors.len()
        );
        std::process::exit(1);
    }

    eprintln!("\nAll tools installed successfully.");
    print_status();
}

// ---------------------------------------------------------------------------
// Helpers used by extract_runner
// ---------------------------------------------------------------------------

/// Resolve charon binary, returning a `Result` for use in `ensure_charon_llbc`.
pub fn resolve_charon_or_err() -> Result<PathBuf, String> {
    resolve_charon().ok_or_else(|| {
        "charon not found. Install it with: probe-aeneas setup\n  \
         Charon is needed for rust-qualified-name enrichment (Aeneas integration)."
            .to_string()
    })
}

/// Find probe-rust on PATH or in `~/.cargo/bin/`, installing if not found.
pub fn find_or_install_probe_rust() -> Result<PathBuf, String> {
    if let Some(p) = resolve_probe_rust() {
        return Ok(p);
    }
    install_probe_rust()
}
