//! `setup` subcommand: install and manage external tool dependencies.
//!
//! Manages probe-rust and charon. After installing probe-rust, delegates to
//! `probe-rust setup` to install its own dependencies (rust-analyzer, scip).
//! probe-lean is version-matched to each target project's `lean-toolchain`
//! and is auto-installed per-project during `extract`, so it is not handled
//! here.
//!
//! ## Error model
//!
//! Library functions return [`Result<T, SetupError>`]. The `SetupError` enum
//! (defined with `thiserror`) names the categorical failure modes. Open-ended
//! IO failures flow through `Other(#[from] anyhow::Error)` via `.with_context()`
//! chains, consistent with every other module in this crate.
//!
//! The [`cmd_setup`] orchestrator uses `anyhow::Error` to aggregate errors
//! from multiple subtasks with `.context()` labels, demonstrating the
//! typical `thiserror`-for-leaves / `anyhow`-for-context split.

use std::path::PathBuf;
use std::process::Command;

use anyhow::Context as _;

const PROBE_RUST_GIT: &str = "https://github.com/Beneficial-AI-Foundation/probe-rust.git";
const CHARON_REPO: &str = "https://github.com/AeneasVerif/charon.git";

// ---------------------------------------------------------------------------
// Typed error
// ---------------------------------------------------------------------------

/// Errors produced by the `setup` module.
///
/// Variants are categorical (callers can match on them); the catch-all
/// [`SetupError::Io`] carries a context string plus the underlying
/// `io::Error` as its `#[source]`.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("could not determine home directory")]
    HomeDirUnavailable,

    #[error(
        "charon not found. Install it with: probe-aeneas setup\n  \
         Charon is needed for rust-qualified-name enrichment (Aeneas integration)."
    )]
    CharonNotFound,

    #[error("cargo install succeeded but probe-rust binary not found in ~/.cargo/bin/")]
    ProbeRustNotFound,

    #[error(
        "cargo install probe-rust failed. Please install manually:\n  \
         cargo install --git https://github.com/Beneficial-AI-Foundation/probe-rust.git"
    )]
    ProbeRustInstallFailed,

    #[error("git clone charon failed")]
    CharonCloneFailed,

    #[error("cargo build --release charon failed")]
    CharonBuildFailed,

    #[error("probe-rust setup failed. Run it manually for details:\n  probe-rust setup")]
    ProbeRustSetupFailed,

    #[error("rustup component add rust-analyzer failed for {toolchain} toolchain: {stderr}")]
    RustupComponentFailed { toolchain: String, stderr: String },

    /// Catch-all wrapping `anyhow::Error`. Covers `io::Error` from spawning
    /// subprocesses and filesystem operations, chained via `.with_context()`.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience alias used throughout this module.
pub type Result<T> = std::result::Result<T, SetupError>;

// ---------------------------------------------------------------------------
// Public installation functions
// ---------------------------------------------------------------------------

/// Clone and build charon from source into `~/.probe-rust/tools/`.
///
/// Mirrors probe-rust's `tool_manager::build_charon` so both tools share the
/// same managed binary. Both `charon` and `charon-driver` are installed.
/// Reuses existing source checkout if present.
pub fn install_charon() -> Result<()> {
    let tools_dir = home_dir()?.join(".probe-rust/tools");
    std::fs::create_dir_all(&tools_dir)
        .with_context(|| format!("create directory {}", tools_dir.display()))?;

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
            .context("spawn `git clone` for charon")?;
        if !status.success() {
            return Err(SetupError::CharonCloneFailed);
        }
    }

    eprintln!("Building charon (this may take a few minutes)...");
    let status = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(src_dir.join("charon"))
        .status()
        .context("spawn `cargo build --release` for charon")?;
    if !status.success() {
        return Err(SetupError::CharonBuildFailed);
    }

    let release_dir = src_dir.join("charon/target/release");
    for binary in ["charon", "charon-driver"] {
        let src = release_dir.join(binary);
        let dst = tools_dir.join(binary);
        std::fs::copy(&src, &dst).with_context(|| format!("copy {binary} to {}", dst.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755))
                .with_context(|| format!("set permissions on {binary}"))?;
        }
    }

    eprintln!("  ✓ Installed charon to {}", tools_dir.display());
    Ok(())
}

/// Install probe-rust via `cargo install --git`.
pub fn install_probe_rust() -> Result<PathBuf> {
    let cargo_bin = home_dir()?.join(".cargo/bin/probe-rust");
    if cargo_bin.exists() {
        return Ok(cargo_bin);
    }

    eprintln!("Installing probe-rust via cargo...");
    let status = Command::new("cargo")
        .args(["install", "--git", PROBE_RUST_GIT])
        .status()
        .context("spawn `cargo install` for probe-rust")?;

    if !status.success() {
        return Err(SetupError::ProbeRustInstallFailed);
    }

    if cargo_bin.exists() {
        Ok(cargo_bin)
    } else {
        Err(SetupError::ProbeRustNotFound)
    }
}

// ---------------------------------------------------------------------------
// Resolution helpers (shared with extract_runner)
// ---------------------------------------------------------------------------

pub fn find_on_path(name: &str) -> Option<PathBuf> {
    which::which(name).ok()
}

pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().ok_or(SetupError::HomeDirUnavailable)
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
pub fn ensure_rust_analyzer_component(toolchain: Option<&str>) -> Result<()> {
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
        .context("spawn `rustup component add rust-analyzer`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(SetupError::RustupComponentFailed {
            toolchain: label.to_string(),
            stderr,
        });
    }
    eprintln!("  ✓ rust-analyzer available for {label} toolchain");
    Ok(())
}

/// Delegate to `probe-rust setup` to install probe-rust's own dependencies
/// (rust-analyzer, scip). The `probe_rust_bin` must already be installed.
fn run_probe_rust_setup(probe_rust_bin: &std::path::Path) -> Result<()> {
    eprintln!("\nRunning probe-rust setup to install its dependencies...\n");
    let status = Command::new(probe_rust_bin)
        .arg("setup")
        .status()
        .context("spawn `probe-rust setup`")?;
    if !status.success() {
        return Err(SetupError::ProbeRustSetupFailed);
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

fn query_tool_version(bin: &std::path::Path) -> Option<String> {
    let output = Command::new(bin).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next()?.trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

fn status_line(name: &str, location: &Option<PathBuf>, note: &str) {
    match location {
        Some(p) => {
            let version = query_tool_version(p).unwrap_or_else(|| "unknown version".to_string());
            eprintln!("  {name:<16} {version}");
            eprintln!("  {:<16} {}", "", p.display());
        }
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
///
/// Stays infallible at the function level (calls `process::exit(1)` on
/// failures) so `main.rs` doesn't need to change. Internally uses
/// `anyhow::Error` to aggregate errors from independent subtasks with
/// `.context()` labels — installing one tool shouldn't abort the rest.
pub fn cmd_setup(status: bool) {
    if status {
        print_status();
        return;
    }

    eprintln!("Installing external tools for probe-aeneas...\n");

    let mut errors: Vec<anyhow::Error> = Vec::new();

    // probe-rust (install if needed, then delegate to its setup for deps)
    let probe_rust_bin = match resolve_probe_rust() {
        Some(p) => {
            eprintln!("probe-rust: already available at {}", p.display());
            Some(p)
        }
        None => match install_probe_rust().context("probe-rust") {
            Ok(p) => Some(p),
            Err(e) => {
                errors.push(e);
                None
            }
        },
    };

    // rust-analyzer + scip (delegated to probe-rust setup)
    if let Some(ref bin) = probe_rust_bin {
        if let Err(e) = run_probe_rust_setup(bin).context("probe-rust dependencies") {
            errors.push(e);
        }
    }

    // Ensure rust-analyzer is installed for the default toolchain.
    // probe-rust setup only *checks* for it (warning, not error), so we
    // install the rustup component directly as a fallback.
    if let Err(e) = ensure_rust_analyzer_component(None).context("rust-analyzer") {
        errors.push(e);
    }

    // charon
    match resolve_charon() {
        Some(p) => eprintln!("charon: already available at {}", p.display()),
        None => {
            if let Err(e) = install_charon().context("charon") {
                errors.push(e);
            }
        }
    }

    if !errors.is_empty() {
        for e in &errors {
            // `{:#}` walks the anyhow context chain (and the underlying
            // SetupError's `#[source]`) so the operator sees the full
            // cause, not just the top-level label.
            eprintln!("Error: {e:#}");
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

/// Resolve charon binary, returning a typed error for use in
/// `ensure_charon_llbc`.
pub fn resolve_charon_or_err() -> Result<PathBuf> {
    resolve_charon().ok_or(SetupError::CharonNotFound)
}

/// Find probe-rust on PATH or in `~/.cargo/bin/`, installing if not found.
pub fn find_or_install_probe_rust() -> Result<PathBuf> {
    if let Some(p) = resolve_probe_rust() {
        return Ok(p);
    }
    install_probe_rust()
}
