//! Runs the external `probe-rust` and `probe-lean` extractors, installs them
//! on demand, and pre-generates the Charon LLBC for Aeneas projects.
//!
//! ## Error model
//!
//! Public and internal functions return [`Result<T, ExtractRunnerError>`].
//! Categorical failure modes (subprocess exit, missing output file, non-UTF-8
//! path, etc.) are typed variants so callers can inspect them. Open-ended
//! io-shaped failures use `.context("...")?` from `anyhow::Context` and are
//! captured by the `Other(#[from] anyhow::Error)` variant — this is where
//! `anyhow` carries its weight inside this module.
//!
//! Errors from the `setup` module propagate through the `Setup` variant via
//! `#[from]`, so `setup::install_charon()?` and similar calls work directly.
//!
//! An `impl From<ExtractRunnerError> for String` at the bottom walks the
//! source chain so callers in `extract.rs`, `main.rs`, and `listfuns.rs`
//! that still return `Result<_, String>` keep working through `?`. It will
//! be removed once those callers migrate.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context as _;

use crate::extract::CharonConfig;
use crate::setup;

const PROBE_LEAN_GIT: &str = "https://github.com/Beneficial-AI-Foundation/probe-lean.git";

/// Opt-in env var that permits the floating-`main` source-build fallback.
/// Unset (the default) makes probe-aeneas release-only: if no tagged pre-built
/// binary matches the project's Lean version, installation fails clearly
/// instead of silently building unreleased code. See issue #46 (#6).
const ALLOW_SOURCE_BUILD_ENV: &str = "PROBE_LEAN_ALLOW_SOURCE_BUILD";

// ---------------------------------------------------------------------------
// Typed error
// ---------------------------------------------------------------------------

/// Errors produced by the `extract_runner` module.
///
/// Categorical variants (`SubprocessFailed`, `MissingOutput`, …) carry the
/// structured fields callers might want to inspect. Generic io-shaped
/// failures flow through `Other(anyhow::Error)` via `.context("...")?`.
#[derive(Debug, thiserror::Error)]
pub enum ExtractRunnerError {
    /// A subprocess exited with a non-zero status code.
    #[error("{command} exited with status {code}")]
    SubprocessFailed { command: String, code: i32 },

    /// A subprocess completed but the expected output file is missing.
    #[error("{command} completed but {} was not created", path.display())]
    MissingOutput { command: String, path: PathBuf },

    /// A path could not be represented as UTF-8.
    #[error("{label} path is not valid UTF-8")]
    NonUtf8Path { label: &'static str },

    /// `lean-toolchain` file is empty.
    #[error("lean-toolchain file is empty")]
    LeanToolchainEmpty,

    /// `lean-toolchain` version string is unusable as a path component
    /// (contains a path separator or `..`).
    #[error("lean-toolchain version {version:?} contains invalid path characters")]
    LeanToolchainInvalid { version: String },

    /// No pre-built binary available for the requested platform/version.
    /// Callers should fall back to building from source.
    #[error("No pre-built binary available, falling back to source build")]
    NoPrebuiltAvailable,

    /// A downloaded archive contains an entry with an absolute path or a `..`
    /// component that would escape the extraction directory.
    #[error("refusing to extract archive: unsafe entry path {entry:?}")]
    UnsafeArchiveEntry { entry: String },

    /// No pre-built release matched the target Lean version and the floating
    /// `main` source-build fallback was not explicitly enabled.
    #[error(
        "no pre-built probe-lean available for {target} on this platform, and building \
         from source is disabled.\n  \
         The source build clones probe-lean's unpinned `main` branch, which is not \
         reproducible and may run unreleased code.\n  \
         To allow it anyway, re-run with {env}=1 in the environment."
    )]
    SourceBuildDisabled { target: String, env: &'static str },

    /// `lake build` failed during source installation of probe-lean.
    #[error(
        "lake build failed (exit {code}).\n  \
         Make sure elan/lean4 and lake are installed: https://github.com/leanprover/elan\n  \
         stderr: {stderr}"
    )]
    LakeBuildFailed { code: i32, stderr: String },

    /// Errors propagated from the `setup` module.
    #[error(transparent)]
    Setup(#[from] setup::SetupError),

    /// Catch-all for context-chained internal errors built via `anyhow`.
    ///
    /// Any `Result<_, io::Error>` (or `Option<T>`) can be turned into this
    /// variant by calling `.context("...")?` from the `anyhow::Context`
    /// trait — `?` converts the resulting `anyhow::Error` through `#[from]`.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience alias used throughout this module.
pub type Result<T> = std::result::Result<T, ExtractRunnerError>;

// ---------------------------------------------------------------------------
// Public extractor entry points
// ---------------------------------------------------------------------------

/// Run `probe-rust extract` on a project and return the path to the generated JSON.
///
/// When `output_dir` is provided, the output file is written there
/// (e.g. `.verilib/probes/`); otherwise a temp file is used.
pub fn run_probe_rust_extract(
    project: &Path,
    output_dir: Option<&Path>,
    with_public_api: bool,
    translation: Option<&Path>,
) -> Result<PathBuf> {
    let bin = find_or_install_probe_rust()?;
    ensure_rust_analyzer_for_project(project);
    let output = output_path(output_dir, "rust_extract", ".json");

    println!("Running probe-rust extract on {}...", project.display());
    let mut args = vec![
        "extract".to_string(),
        project
            .to_str()
            .ok_or(ExtractRunnerError::NonUtf8Path { label: "Project" })?
            .to_string(),
        "-o".to_string(),
        output
            .to_str()
            .ok_or(ExtractRunnerError::NonUtf8Path { label: "Output" })?
            .to_string(),
        "--auto-install".to_string(),
    ];
    // When an Aeneas manifest is available, probe-rust reads charon `def_id`s
    // from it (no charon run); otherwise it runs charon for the LLBC (legacy).
    match translation {
        Some(tj) => {
            args.push("--translation".to_string());
            args.push(
                tj.to_str()
                    .ok_or(ExtractRunnerError::NonUtf8Path {
                        label: "Translation",
                    })?
                    .to_string(),
            );
        }
        None => args.push("--with-charon".to_string()),
    }
    if with_public_api {
        args.push("--with-public-api".to_string());
    }
    let status = Command::new(&bin)
        .args(&args)
        .status()
        .context("spawn `probe-rust`")?;

    if !status.success() {
        return Err(ExtractRunnerError::SubprocessFailed {
            command: "probe-rust extract".to_string(),
            code: status.code().unwrap_or(-1),
        });
    }

    if !output.exists() {
        return Err(ExtractRunnerError::MissingOutput {
            command: "probe-rust extract".to_string(),
            path: output,
        });
    }

    println!("  ✓ Rust atoms: {}", output.display());
    Ok(output)
}

/// Run `probe-lean extract` on a project and return the path to the generated JSON.
///
/// When `output_dir` is provided, the output file is written there
/// (e.g. `.verilib/probes/`); otherwise a temp file is used.
pub fn run_probe_lean_extract(project: &Path, output_dir: Option<&Path>) -> Result<PathBuf> {
    run_probe_lean_extract_with_opts(project, None, output_dir)
}

/// Run `probe-lean extract` with optional module prefix filter.
pub fn run_probe_lean_extract_with_opts(
    project: &Path,
    module_prefix: Option<&str>,
    output_dir: Option<&Path>,
) -> Result<PathBuf> {
    let bin = find_or_install_probe_lean(Some(project))?;
    let output = output_path(output_dir, "lean_extract", ".json");

    let project_str = project
        .to_str()
        .ok_or(ExtractRunnerError::NonUtf8Path { label: "Project" })?;
    let output_str = output
        .to_str()
        .ok_or(ExtractRunnerError::NonUtf8Path { label: "Output" })?;

    let mut args = vec!["extract", project_str, "-o", output_str];
    let module_flag;
    if let Some(m) = module_prefix {
        module_flag = m.to_string();
        args.push("-m");
        args.push(&module_flag);
    }

    println!("Running probe-lean extract on {}...", project.display());
    let status = Command::new(&bin)
        .args(&args)
        .status()
        .context("spawn `probe-lean`")?;

    if !status.success() {
        return Err(ExtractRunnerError::SubprocessFailed {
            command: "probe-lean extract".to_string(),
            code: status.code().unwrap_or(-1),
        });
    }

    if !output.exists() {
        return Err(ExtractRunnerError::MissingOutput {
            command: "probe-lean extract".to_string(),
            path: output,
        });
    }

    println!("  ✓ Lean atoms: {}", output.display());
    Ok(output)
}

// ---------------------------------------------------------------------------
// Installer plumbing
// ---------------------------------------------------------------------------

fn find_or_install_probe_rust() -> Result<PathBuf> {
    // SetupError -> ExtractRunnerError::Setup via #[from].
    Ok(setup::find_or_install_probe_rust()?)
}

fn find_or_install_probe_lean(lean_project: Option<&Path>) -> Result<PathBuf> {
    // A missing `lean-toolchain` is tolerable (fall through to the unversioned
    // "latest" install), but an empty/unreadable/malformed one is a hard error:
    // silently installing an unversioned probe-lean can produce an incompatible
    // .olean format whose failure only surfaces much later downstream (#46 #3).
    let lean_version = match lean_project {
        Some(p) => detect_lean_version(p)?,
        None => None,
    };

    if let Some(ref ver) = lean_version {
        let versioned_bin = home_dir()?.join(format!(".local/bin/probe-lean-{ver}"));
        if versioned_bin.exists() {
            println!("Using versioned probe-lean for Lean {ver}");
            return Ok(versioned_bin);
        }
        // Specific version required but not installed — skip unversioned
        // fallbacks (PATH, symlink) since they may point to an incompatible
        // Lean version with a different olean format.
    } else {
        if let Some(p) = find_on_path("probe-lean") {
            return Ok(p);
        }
        let local_bin = home_dir()?.join(".local/bin/probe-lean");
        if local_bin.exists() {
            return Ok(local_bin);
        }
    }

    let version = lean_version.unwrap_or_else(|| "latest".to_string());
    println!("probe-lean not found for Lean {version}, installing...");

    let dest_dir = home_dir()?.join(".local/bin");
    std::fs::create_dir_all(&dest_dir).context("create ~/.local/bin")?;

    if version != "latest" {
        if let Ok(bin) = try_prebuilt_download(&version) {
            update_symlink(&bin)?;
            return Ok(bin);
        }
    }

    build_from_source(&version)
}

/// Read the Lean version from a project's `lean-toolchain` file.
///
/// Returns `Ok(None)` when the file simply does not exist (the caller may then
/// fall through to an unversioned install). A file that exists but is empty,
/// unreadable (permissions), or otherwise unparseable is a hard error, not a
/// silent `None` — see [`find_or_install_probe_lean`] and issue #46 (#3).
fn detect_lean_version(project: &Path) -> Result<Option<String>> {
    let toolchain_path = project.join("lean-toolchain");
    let content = match std::fs::read_to_string(&toolchain_path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::Error::new(e)
                .context(format!(
                    "read lean-toolchain at {}",
                    toolchain_path.display()
                ))
                .into());
        }
    };
    let trimmed = content.trim();
    let version = if let Some(after_colon) = trimmed.split(':').nth(1) {
        after_colon.trim().to_string()
    } else {
        trimmed.to_string()
    };
    if version.is_empty() {
        return Err(ExtractRunnerError::LeanToolchainEmpty);
    }
    // The version is interpolated into install paths (`probe-lean-{ver}`) and the
    // release artifact name, so reject anything that isn't a safe path component
    // (#46 #7). `lean-toolchain` is project-controlled input.
    if version.contains('/') || version.contains('\\') || version.contains("..") {
        return Err(ExtractRunnerError::LeanToolchainInvalid { version });
    }
    Ok(Some(version))
}

/// Detect platform as `{os}-{arch}` for pre-built binary downloads.
fn detect_platform() -> String {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "unknown"
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unknown"
    };
    format!("{os}-{arch}")
}

/// Try downloading a pre-built probe-lean binary from GitHub Releases.
fn try_prebuilt_download(lean_version: &str) -> Result<PathBuf> {
    let platform = detect_platform();
    let artifact = format!("probe-lean-{lean_version}-{platform}.tar.gz");
    println!("Checking for pre-built binary: {artifact}...");

    let output = Command::new("curl")
        .args([
            "-sL",
            "https://api.github.com/repos/Beneficial-AI-Foundation/probe-lean/releases",
        ])
        .output()
        .context("query GitHub releases via curl")?;

    if !output.status.success() {
        return Err(anyhow::anyhow!("GitHub API request failed").into());
    }

    let body = String::from_utf8_lossy(&output.stdout);
    let url =
        find_release_asset_url(&body, &artifact).ok_or(ExtractRunnerError::NoPrebuiltAvailable)?;

    println!("Downloading pre-built binary...");

    // A unique, auto-cleaned temp dir: two concurrent extract runs no longer
    // share (and clobber) a fixed `/tmp/probe-lean-download` path (#46 #2).
    let tmpdir = tempfile::TempDir::new().context("create temp dir for probe-lean download")?;
    let tmp = tmpdir.path();
    let archive = tmp.join("probe-lean.tar.gz");

    // Download to a file first (not a raw `curl | tar` pipe) so the archive can
    // be inspected before anything is written to disk (#46 #5). `--fail` turns
    // HTTP errors into a non-zero exit instead of a saved error page.
    // NOTE: there is still no checksum/signature verification here — that needs
    // probe-lean releases to publish checksums; tracked as a follow-up on #46.
    let status = Command::new("curl")
        .args(["--fail", "-sL", "-o"])
        .arg(&archive)
        .arg(&url)
        .status()
        .context("download probe-lean archive via curl")?;
    if !status.success() {
        return Err(anyhow::anyhow!(
            "Download failed (curl exit {})",
            status.code().unwrap_or(-1)
        )
        .into());
    }

    // Reject archives whose entries would escape the extraction directory via
    // absolute paths or `..` components before extracting anything (#46 #5).
    let listing = Command::new("tar")
        .arg("-tzf")
        .arg(&archive)
        .output()
        .context("list probe-lean archive contents")?;
    if !listing.status.success() {
        return Err(anyhow::anyhow!("Failed to list archive contents").into());
    }
    for entry in String::from_utf8_lossy(&listing.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        if !is_safe_archive_entry(entry) {
            return Err(ExtractRunnerError::UnsafeArchiveEntry {
                entry: entry.to_string(),
            });
        }
    }

    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(tmp)
        .status()
        .context("extract probe-lean archive")?;
    if !status.success() {
        return Err(anyhow::anyhow!("Extraction failed").into());
    }

    let dest_dir = home_dir()?.join(".local/bin");
    std::fs::create_dir_all(&dest_dir).context("create ~/.local/bin")?;

    let versioned_bin = dest_dir.join(format!("probe-lean-{lean_version}"));
    let downloaded_bin = tmp.join("bin/probe-lean");
    // Require a regular file: a symlinked `bin/probe-lean` would make the copy
    // below follow it out of the extracted tree (#46 #2).
    match std::fs::symlink_metadata(&downloaded_bin) {
        Ok(m) if m.file_type().is_file() => {}
        Ok(_) => {
            return Err(ExtractRunnerError::UnsafeArchiveEntry {
                entry: "bin/probe-lean".to_string(),
            });
        }
        Err(_) => {
            return Err(
                anyhow::anyhow!("Downloaded archive does not contain bin/probe-lean").into(),
            );
        }
    }

    install_file_atomic(&downloaded_bin, &versioned_bin)?;

    let versioned_lib = home_dir()?.join(format!(".local/lib/probe-lean-{lean_version}"));
    let downloaded_lib = tmp.join("lib");
    if downloaded_lib.exists() {
        install_dir_atomic(&downloaded_lib, &versioned_lib)?;
    }

    // `tmpdir` (the TempDir) is dropped here, cleaning up the extraction dir.
    println!("  ✓ Installed pre-built probe-lean-{lean_version}");
    Ok(versioned_bin)
}

/// Find the `browser_download_url` for an asset named exactly `artifact` in a
/// GitHub releases API JSON response.
///
/// Parses the response with `serde_json` and matches the asset `name` field
/// exactly, rather than line-grepping for `browser_download_url` (which was
/// fragile to response formatting and could match an unintended asset — #46 #4).
fn find_release_asset_url(body: &str, artifact: &str) -> Option<String> {
    let releases: serde_json::Value = serde_json::from_str(body).ok()?;
    for release in releases.as_array()? {
        let Some(assets) = release.get("assets").and_then(|a| a.as_array()) else {
            continue;
        };
        for asset in assets {
            if asset.get("name").and_then(|n| n.as_str()) == Some(artifact) {
                if let Some(url) = asset.get("browser_download_url").and_then(|u| u.as_str()) {
                    return Some(url.to_string());
                }
            }
        }
    }
    None
}

/// Reject archive entry names that would escape the extraction directory:
/// absolute paths (`/etc/...`) or any `..` path component.
fn is_safe_archive_entry(entry: &str) -> bool {
    let path = Path::new(entry);
    if path.is_absolute() {
        return false;
    }
    !path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Whether the floating-`main` source-build fallback is explicitly enabled via
/// [`ALLOW_SOURCE_BUILD_ENV`]. Fails closed: only recognized truthy values
/// enable it; an unrecognized value warns and stays disabled.
fn source_build_allowed() -> bool {
    match std::env::var(ALLOW_SOURCE_BUILD_ENV) {
        Ok(v) => env_flag_enabled(&v),
        Err(_) => false,
    }
}

/// Interpret an env-var string as an on/off flag, failing closed. Only `1`,
/// `true`, `yes`, and `on` (case-insensitive, trimmed) enable; empty/`0`/
/// `false`/`no` disable silently; anything else disables with a warning, so a
/// safety gate is never accidentally opened by `off`, a typo, etc. (#46 #5).
fn env_flag_enabled(value: &str) -> bool {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "" | "0" | "false" | "no" => false,
        other => {
            eprintln!(
                "  ⚠ {ALLOW_SOURCE_BUILD_ENV}={other:?} is not a recognized on/off value; \
                 treating it as disabled. Use 1/true/yes/on to enable."
            );
            false
        }
    }
}

/// Build probe-lean from source for a specific Lean version.
///
/// This clones probe-lean's unpinned `main` branch, so the result is not
/// reproducible and may include unreleased code. It is therefore gated behind
/// [`ALLOW_SOURCE_BUILD_ENV`]: unless that env var is set, the function returns
/// [`ExtractRunnerError::SourceBuildDisabled`] so release-only consumers (e.g.
/// verilib) fail clearly instead of silently building `main` (#46 #6).
fn build_from_source(lean_version: &str) -> Result<PathBuf> {
    if !source_build_allowed() {
        // "latest" means no lean-toolchain was found to select a release, so
        // don't phrase the error as "no release for Lean latest" (#46 #4).
        let target = if lean_version == "latest" {
            "this project (no lean-toolchain found to select a pre-built release)".to_string()
        } else {
            format!("Lean {lean_version}")
        };
        return Err(ExtractRunnerError::SourceBuildDisabled {
            target,
            env: ALLOW_SOURCE_BUILD_ENV,
        });
    }

    println!("Building probe-lean from source for Lean {lean_version}...");
    eprintln!(
        "  ⚠ {ALLOW_SOURCE_BUILD_ENV} is set: building probe-lean from the unpinned `main` \
         branch. This is not reproducible and may run unreleased code."
    );

    // Unique, auto-cleaned build dir so concurrent runs don't clobber a shared
    // `/tmp/probe-lean-build` path (#46 #2).
    let build_tmp = tempfile::TempDir::new().context("create temp dir for probe-lean build")?;
    let build_dir = build_tmp.path();

    let status = Command::new("git")
        .args(["clone", "--depth", "1", PROBE_LEAN_GIT])
        .arg(build_dir)
        .status()
        .context("spawn `git clone` for probe-lean")?;

    if !status.success() {
        return Err(anyhow::anyhow!("git clone probe-lean failed").into());
    }

    if lean_version != "latest" {
        let toolchain_content = format!("leanprover/lean4:{lean_version}\n");
        std::fs::write(build_dir.join("lean-toolchain"), &toolchain_content)
            .context("write lean-toolchain pin")?;

        let lake_manifest = build_dir.join("lake-manifest.json");
        if lake_manifest.exists() {
            std::fs::remove_file(&lake_manifest).ok();
        }
        let lake_dir = build_dir.join(".lake");
        if lake_dir.exists() {
            std::fs::remove_dir_all(&lake_dir).ok();
        }
    }

    let output = Command::new("lake")
        .arg("build")
        .current_dir(build_dir)
        .output()
        .context("spawn `lake build`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(ExtractRunnerError::LakeBuildFailed {
            code: output.status.code().unwrap_or(-1),
            stderr,
        });
    }

    let built_bin = build_dir.join(".lake/build/bin/probe-lean");
    if !built_bin.exists() {
        return Err(ExtractRunnerError::MissingOutput {
            command: "lake build".to_string(),
            path: built_bin,
        });
    }

    let dest_dir = home_dir()?.join(".local/bin");
    std::fs::create_dir_all(&dest_dir).context("create ~/.local/bin")?;

    // Always install to a versioned name and route the bare `probe-lean` path
    // through update_symlink's non-clobber guard — including the "latest" case,
    // which previously copied straight onto `~/.local/bin/probe-lean` and could
    // silently destroy a user-installed binary (#46 #1).
    let versioned_name = if lean_version != "latest" {
        format!("probe-lean-{lean_version}")
    } else {
        "probe-lean-latest".to_string()
    };
    let dest_bin = dest_dir.join(&versioned_name);

    install_file_atomic(&built_bin, &dest_bin)?;
    update_symlink(&dest_bin)?;

    println!("  ✓ Installed {versioned_name} to {}", dest_bin.display());
    Ok(dest_bin)
}

/// Update the `~/.local/bin/probe-lean` symlink to point at a versioned binary.
///
/// Only ever replaces an existing symlink; a regular file (or directory) at the
/// path is left untouched with a warning, so a user-installed or
/// package-manager-owned `probe-lean` is never silently destroyed (#46 #1). The
/// replacement is atomic (temp symlink + rename), so a concurrent reader never
/// observes a missing link (#46 #3).
#[cfg(unix)]
fn update_symlink(versioned_bin: &Path) -> Result<()> {
    let symlink = versioned_bin
        .parent()
        .context("versioned binary has no parent directory")?
        .join("probe-lean");

    // `symlink_metadata` does not follow the link, so this distinguishes an
    // existing symlink from a real file we must not clobber.
    match std::fs::symlink_metadata(&symlink) {
        Ok(meta) if meta.file_type().is_symlink() => {}
        Ok(_) => {
            eprintln!(
                "  ⚠ {} exists and is not a symlink managed by probe-aeneas; \
                 leaving it untouched. Use the versioned binary directly, or remove that \
                 file yourself to let probe-aeneas manage the symlink.",
                symlink.display()
            );
            return Ok(());
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(anyhow::Error::new(e)
                .context(format!("stat {}", symlink.display()))
                .into());
        }
    }

    let target = versioned_bin
        .file_name()
        .context("versioned binary has no filename")?;
    let tmp = symlink.with_file_name(format!(".probe-lean.tmp-{}", unique_suffix()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)
        .with_context(|| format!("create symlink at {}", tmp.display()))?;
    // rename atomically replaces the old symlink (if any) in one step.
    if let Err(e) = std::fs::rename(&tmp, &symlink) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::Error::new(e)
            .context(format!("install symlink at {}", symlink.display()))
            .into());
    }
    Ok(())
}

/// Non-unix fallback: there are no POSIX symlinks, so install a copy of the
/// versioned binary at the bare `probe-lean` path. There is no managed-symlink
/// distinction to make here, so this always overwrites its own prior copy
/// rather than refusing to update it (#46 #8).
#[cfg(not(unix))]
fn update_symlink(versioned_bin: &Path) -> Result<()> {
    let dest = versioned_bin
        .parent()
        .context("versioned binary has no parent directory")?
        .join("probe-lean");
    install_file_atomic(versioned_bin, &dest)
}

/// Per-process counter for unique staging names (`Date`/random are unavailable).
static INSTALL_SEQ: AtomicU64 = AtomicU64::new(0);

/// A process-unique suffix for staging temp files/dirs on the destination
/// filesystem: pid (unique across concurrent processes) plus a monotonic
/// counter (unique across threads/calls within one process).
fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        INSTALL_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// Atomically install `src` as an executable at `dest`: copy to a temp file in
/// the same directory, mark it `+x`, then rename over `dest`. The rename is
/// atomic on one filesystem, so a concurrent reader never sees a half-written
/// binary and two concurrent installs of the same version can't tear (#46 #3).
fn install_file_atomic(src: &Path, dest: &Path) -> Result<()> {
    let dir = dest
        .parent()
        .context("install destination has no parent directory")?;
    let file_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .context("install destination has no filename")?;
    let tmp = dir.join(format!(".{file_name}.tmp-{}", unique_suffix()));
    let _ = std::fs::remove_file(&tmp);

    let staged = (|| -> Result<()> {
        std::fs::copy(src, &tmp)
            .with_context(|| format!("copy {} to {}", src.display(), tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
                .with_context(|| format!("set +x on {}", tmp.display()))?;
        }
        std::fs::rename(&tmp, dest)
            .with_context(|| format!("install {} -> {}", tmp.display(), dest.display()))?;
        Ok(())
    })();

    if staged.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    staged
}

/// Atomically install directory `src` at `dest`: stage a copy into a temp dir on
/// the same filesystem, then swap it into place (moving any existing `dest`
/// aside first). Concurrent readers see either the old or the new tree, never a
/// half-copied one (#46 #3).
fn install_dir_atomic(src: &Path, dest: &Path) -> Result<()> {
    let parent = dest
        .parent()
        .context("install destination has no parent directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;

    let staging = parent.join(format!(".probe-lean-lib.tmp-{}", unique_suffix()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("create staging dir {}", staging.display()))?;

    if let Err(e) = copy_dir_contents(src, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    if dest.exists() {
        // Move the existing tree aside, swap the new one in, then drop the old.
        // On failure, restore the old tree so `dest` is never left missing.
        let backup = parent.join(format!(".probe-lean-lib.old-{}", unique_suffix()));
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::rename(dest, &backup).with_context(|| format!("move aside {}", dest.display()))?;
        if let Err(e) = std::fs::rename(&staging, dest) {
            let _ = std::fs::rename(&backup, dest);
            let _ = std::fs::remove_dir_all(&staging);
            return Err(anyhow::Error::new(e)
                .context(format!("install lib dir -> {}", dest.display()))
                .into());
        }
        let _ = std::fs::remove_dir_all(&backup);
    } else if let Err(e) = std::fs::rename(&staging, dest) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(anyhow::Error::new(e)
            .context(format!("install lib dir -> {}", dest.display()))
            .into());
    }
    Ok(())
}

/// Recursively copy directory contents from `src` to `dst`.
///
/// Rejects symlink (and other non-regular) entries rather than following them,
/// so a crafted archive can't make the copy read files outside the extracted
/// tree (#46 #2). This includes the `src` root itself: `read_dir` follows a
/// symlinked root, and the per-entry checks below only guard children, so the
/// root must be verified to be a real directory first.
fn copy_dir_contents(src: &Path, dst: &Path) -> Result<()> {
    let src_meta =
        std::fs::symlink_metadata(src).with_context(|| format!("stat {}", src.display()))?;
    if !src_meta.file_type().is_dir() {
        return Err(ExtractRunnerError::UnsafeArchiveEntry {
            entry: src.display().to_string(),
        });
    }
    let entries = std::fs::read_dir(src).with_context(|| format!("read dir {}", src.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("read entry in {}", src.display()))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        // `file_type()` does not follow symlinks (unlike `Path::is_dir`).
        let ft = entry
            .file_type()
            .with_context(|| format!("stat entry {}", src_path.display()))?;
        if ft.is_symlink() || (!ft.is_dir() && !ft.is_file()) {
            return Err(ExtractRunnerError::UnsafeArchiveEntry {
                entry: src_path.display().to_string(),
            });
        }
        if ft.is_dir() {
            std::fs::create_dir_all(&dst_path)
                .with_context(|| format!("create dir {}", dst_path.display()))?;
            copy_dir_contents(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).with_context(|| {
                format!("copy {} to {}", src_path.display(), dst_path.display())
            })?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Charon LLBC pre-generation
// ---------------------------------------------------------------------------

/// Pre-generate the Charon LLBC file using config from `aeneas-config.yml`.
///
/// `probe-rust --with-charon` runs charon with only `--preset aeneas`, which
/// misses project-specific cargo args (e.g. `--no-default-features`),
/// `--start-from` filters, and `--exclude` lists. This function runs charon
/// directly with the full configuration so the LLBC is cached at
/// `<rust_project>/data/charon.llbc` before `probe-rust` needs it.
///
/// Only used on the legacy (no-manifest) path. When a `translation.json` is
/// present, probe-rust reads charon `def_id`s from the manifest instead of
/// running charon, so this pre-flight is skipped entirely (see
/// [`crate::main`]'s `resolve_and_extract`).
pub fn ensure_charon_llbc(rust_project: &Path, config: &CharonConfig) -> Result<()> {
    let data_dir = rust_project.join("data");
    let llbc_path = data_dir.join("charon.llbc");

    if llbc_path.exists() {
        println!("Using cached Charon LLBC at {}", llbc_path.display());
        return Ok(());
    }

    let charon_bin = match setup::resolve_charon_or_err() {
        Ok(bin) => bin,
        Err(_) => {
            println!("charon not found, building from source...");
            setup::install_charon()?;
            setup::resolve_charon_or_err()?
        }
    };

    std::fs::create_dir_all(&data_dir).with_context(|| format!("create {}", data_dir.display()))?;

    // Canonicalize to absolute path: charon resolves --dest-file relative to
    // its own cwd (rust_project), not probe-aeneas's cwd.
    let abs_llbc = std::fs::canonicalize(&data_dir)
        .with_context(|| format!("canonicalize {}", data_dir.display()))?
        .join("charon.llbc");
    let llbc_str = abs_llbc.to_string_lossy();

    let mut args: Vec<String> = vec![
        "cargo".to_string(),
        "--preset".to_string(),
        config.preset.as_deref().unwrap_or("aeneas").to_string(),
        "--dest-file".to_string(),
        llbc_str.to_string(),
        "--no-dedup-serialized-ast".to_string(),
    ];

    if config.start_from_pub == Some(true) {
        args.push("--start-from-pub".to_string());
    }

    if let Some(ref start_from) = config.start_from {
        for item in start_from {
            args.push("--start-from".to_string());
            args.push(item.clone());
        }
    }

    if let Some(ref include) = config.include {
        for item in include {
            args.push("--include".to_string());
            args.push(item.clone());
        }
    }

    if let Some(ref exclude) = config.exclude {
        for item in exclude {
            args.push("--exclude".to_string());
            args.push(item.clone());
        }
    }

    if let Some(ref opaque) = config.opaque {
        for item in opaque {
            args.push("--opaque".to_string());
            args.push(item.clone());
        }
    }

    if let Some(ref cargo_args) = config.cargo_args {
        args.push("--".to_string());
        if let Some(ref pkg) = config.package {
            args.push("--package".to_string());
            args.push(pkg.clone());
        }
        args.extend(cargo_args.iter().cloned());
    } else if let Some(ref pkg) = config.package {
        args.push("--".to_string());
        args.push("--package".to_string());
        args.push(pkg.clone());
    }

    println!("\nPre-generating Charon LLBC with aeneas-config.yml settings...");

    let mut path_env = std::env::var("PATH").unwrap_or_default();
    if let Some(parent) = charon_bin.parent() {
        path_env = format!("{}:{}", parent.display(), path_env);
    }

    let output = Command::new(&charon_bin)
        .args(&args)
        .current_dir(rust_project)
        .env("PATH", &path_env)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .output()
        .context("spawn `charon`")?;

    if !output.status.success() {
        eprintln!(
            "  ⚠ Charon pre-generation failed (exit {}); \
             probe-rust will retry with defaults",
            output.status.code().unwrap_or(-1)
        );
        return Ok(());
    }

    if !llbc_path.exists() {
        eprintln!("  ⚠ Charon ran successfully but LLBC file was not created");
        return Ok(());
    }

    println!("  ✓ Charon LLBC generated at {}\n", llbc_path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Rust toolchain detection
// ---------------------------------------------------------------------------

/// Detect the Rust project's active toolchain and ensure `rust-analyzer` is
/// installed for it. Falls back silently so extraction can still attempt to
/// proceed (probe-rust may have its own fallback).
fn ensure_rust_analyzer_for_project(project: &Path) {
    let toolchain = detect_rust_toolchain(project);
    let result = match toolchain {
        Some(ref tc) => setup::ensure_rust_analyzer_component(Some(tc)),
        None => setup::ensure_rust_analyzer_component(None),
    };
    if let Err(e) = result {
        // Wrap into anyhow::Error so `{:#}` walks the SetupError source
        // chain — `e` itself only Displays the top-level context, which
        // hides the underlying io::Error for `SetupError::Io` variants.
        eprintln!("Warning: {:#}", anyhow::Error::new(e));
    }
}

/// Read the channel from `rust-toolchain.toml` or `rust-toolchain` in the
/// project (or any ancestor up to the filesystem root).
fn detect_rust_toolchain(project: &Path) -> Option<String> {
    let abs = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    let mut dir = abs.as_path();
    loop {
        if let Some(tc) = try_read_toolchain_toml(&dir.join("rust-toolchain.toml")) {
            return Some(tc);
        }
        if let Some(tc) = try_read_toolchain_file(&dir.join("rust-toolchain")) {
            return Some(tc);
        }
        dir = dir.parent()?;
    }
}

/// Parse `rust-toolchain.toml` for `[toolchain] channel = "..."`.
fn try_read_toolchain_toml(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("channel") {
            if let Some(value) = trimmed.split('=').nth(1) {
                let channel = value.trim().trim_matches('"').trim_matches('\'');
                if !channel.is_empty() {
                    return Some(channel.to_string());
                }
            }
        }
    }
    None
}

/// Parse a bare `rust-toolchain` file (single line with toolchain name).
fn try_read_toolchain_file(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.contains('[') {
        return None;
    }
    Some(trimmed.to_string())
}

// ---------------------------------------------------------------------------
// Small helpers re-exported from `setup`
// ---------------------------------------------------------------------------

fn find_on_path(name: &str) -> Option<PathBuf> {
    setup::find_on_path(name)
}

fn home_dir() -> Result<PathBuf> {
    Ok(setup::home_dir()?)
}

// ---------------------------------------------------------------------------
// Output path resolution
// ---------------------------------------------------------------------------

/// Compute the output path for an extractor. When `output_dir` is given, writes
/// a stable-named file there (e.g. `.verilib/probes/rust_extract.json`);
/// otherwise falls back to a unique temp file.
fn output_path(output_dir: Option<&Path>, name: &str, suffix: &str) -> PathBuf {
    match output_dir {
        Some(dir) => dir.join(format!("{name}{suffix}")),
        None => tempfile(name, suffix),
    }
}

fn tempfile(prefix: &str, suffix: &str) -> PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    std::env::temp_dir().join(format!("{prefix}_{ts}_{pid}{suffix}"))
}

// ---------------------------------------------------------------------------
// Boundary: String compatibility for callers not yet migrated
// ---------------------------------------------------------------------------

/// Convert an `ExtractRunnerError` to a `String` so callers in `extract.rs`,
/// `main.rs`, and `listfuns.rs` that still return `Result<_, String>` can
/// propagate via `?`.
///
/// Walks the `std::error::Error::source` chain so the resulting string
/// matches the pre-migration `format!("Failed to X: {e}")` shape, including
/// for `Other(anyhow::Error)` whose own chain is walked.
impl From<ExtractRunnerError> for String {
    fn from(err: ExtractRunnerError) -> Self {
        use std::error::Error;
        let mut msg = err.to_string();
        let mut source: Option<&dyn Error> = err.source();
        while let Some(s) = source {
            msg.push_str(": ");
            msg.push_str(&s.to_string());
            source = s.source();
        }
        msg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- detect_lean_version: distinguish missing vs. malformed (#46 #3) ---

    #[test]
    fn detect_lean_version_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_lean_version(dir.path()).unwrap().is_none());
    }

    #[test]
    fn detect_lean_version_parses_channel() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lean-toolchain"),
            "leanprover/lean4:v4.15.0\n",
        )
        .unwrap();
        assert_eq!(
            detect_lean_version(dir.path()).unwrap().as_deref(),
            Some("v4.15.0")
        );
    }

    #[test]
    fn detect_lean_version_empty_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lean-toolchain"), "   \n").unwrap();
        assert!(matches!(
            detect_lean_version(dir.path()),
            Err(ExtractRunnerError::LeanToolchainEmpty)
        ));
    }

    #[test]
    fn detect_lean_version_rejects_path_chars() {
        // A version usable as a path component would let project input control
        // install paths (#46 #7).
        for bad in ["leanprover/lean4:../../evil", "leanprover/lean4:a/b"] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("lean-toolchain"), bad).unwrap();
            assert!(
                matches!(
                    detect_lean_version(dir.path()),
                    Err(ExtractRunnerError::LeanToolchainInvalid { .. })
                ),
                "{bad:?} should be rejected"
            );
        }
    }

    // --- find_release_asset_url: JSON parsing, exact name match (#46 #4) ---

    #[test]
    fn find_release_asset_url_exact_match() {
        let body = r#"[
            {"assets": [
                {"name": "probe-lean-v4.15.0-linux-x86_64.tar.gz",
                 "browser_download_url": "https://example.com/a.tar.gz"},
                {"name": "probe-lean-v4.15.0-darwin-arm64.tar.gz",
                 "browser_download_url": "https://example.com/b.tar.gz"}
            ]}
        ]"#;
        assert_eq!(
            find_release_asset_url(body, "probe-lean-v4.15.0-darwin-arm64.tar.gz").as_deref(),
            Some("https://example.com/b.tar.gz")
        );
    }

    #[test]
    fn find_release_asset_url_no_substring_false_positive() {
        // The requested artifact is a substring of an existing asset's name,
        // but not an exact match — the old line-grep would have matched it.
        let body = r#"[
            {"assets": [
                {"name": "probe-lean-v4.15.0-linux-x86_64.tar.gz.sha256",
                 "browser_download_url": "https://example.com/checksum"}
            ]}
        ]"#;
        assert!(find_release_asset_url(body, "probe-lean-v4.15.0-linux-x86_64.tar.gz").is_none());
    }

    #[test]
    fn find_release_asset_url_tolerates_release_without_assets() {
        let body = r#"[
            {"name": "no-assets-release"},
            {"assets": [
                {"name": "wanted.tar.gz",
                 "browser_download_url": "https://example.com/w"}
            ]}
        ]"#;
        assert_eq!(
            find_release_asset_url(body, "wanted.tar.gz").as_deref(),
            Some("https://example.com/w")
        );
    }

    #[test]
    fn find_release_asset_url_invalid_json_is_none() {
        assert!(find_release_asset_url("not json", "x.tar.gz").is_none());
    }

    // --- is_safe_archive_entry: path-traversal guard (#46 #5) ---

    #[test]
    fn safe_archive_entries_accepted() {
        assert!(is_safe_archive_entry("bin/probe-lean"));
        assert!(is_safe_archive_entry("lib/foo/bar.olean"));
        assert!(is_safe_archive_entry("./bin/probe-lean"));
    }

    #[test]
    fn unsafe_archive_entries_rejected() {
        assert!(!is_safe_archive_entry("/etc/passwd"));
        assert!(!is_safe_archive_entry("../escape"));
        assert!(!is_safe_archive_entry("bin/../../escape"));
    }

    // --- env_flag_enabled: source-build gate parsing, fail-closed (#46 #5/#6) ---

    #[test]
    fn env_flag_disabled_values() {
        // Explicit off values AND unrecognized ones (off/disable/typo) must all
        // stay disabled so the safety gate never opens by accident.
        for v in [
            "", "  ", "0", "false", "FALSE", "no", " No ", "off", "disable", "garbage",
        ] {
            assert!(!env_flag_enabled(v), "{v:?} should be disabled");
        }
    }

    #[test]
    fn env_flag_enabled_values() {
        for v in ["1", "true", "yes", "on", " ON "] {
            assert!(env_flag_enabled(v), "{v:?} should be enabled");
        }
    }

    // --- update_symlink: never clobbers a non-symlink (#46 #1) ---

    #[cfg(unix)]
    #[test]
    fn update_symlink_leaves_regular_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let versioned = dir.path().join("probe-lean-v4.15.0");
        std::fs::write(&versioned, b"versioned").unwrap();
        // A pre-existing regular file at the symlink path (e.g. user-installed).
        let occupied = dir.path().join("probe-lean");
        std::fs::write(&occupied, b"user-installed").unwrap();

        update_symlink(&versioned).unwrap();

        // Untouched: still a regular file with the original contents.
        let meta = std::fs::symlink_metadata(&occupied).unwrap();
        assert!(meta.file_type().is_file());
        assert_eq!(std::fs::read(&occupied).unwrap(), b"user-installed");
    }

    #[cfg(unix)]
    #[test]
    fn update_symlink_replaces_existing_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let old_target = dir.path().join("probe-lean-old");
        std::fs::write(&old_target, b"old").unwrap();
        let versioned = dir.path().join("probe-lean-v4.15.0");
        std::fs::write(&versioned, b"new").unwrap();
        let link = dir.path().join("probe-lean");
        std::os::unix::fs::symlink("probe-lean-old", &link).unwrap();

        update_symlink(&versioned).unwrap();

        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(meta.file_type().is_symlink());
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            Path::new("probe-lean-v4.15.0")
        );
    }

    #[cfg(unix)]
    #[test]
    fn update_symlink_creates_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let versioned = dir.path().join("probe-lean-v4.15.0");
        std::fs::write(&versioned, b"new").unwrap();

        update_symlink(&versioned).unwrap();

        let link = dir.path().join("probe-lean");
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    // --- atomic install helpers (#46 #3) ---

    #[cfg(unix)]
    #[test]
    fn install_file_atomic_replaces_and_sets_exec() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("built");
        std::fs::write(&src, b"payload").unwrap();
        let dest = dir.path().join("probe-lean-v4.15.0");
        std::fs::write(&dest, b"stale").unwrap();

        install_file_atomic(&src, &dest).unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
        assert_ne!(
            std::fs::metadata(&dest).unwrap().permissions().mode() & 0o111,
            0
        );
        // No staging temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "staging temp left behind");
    }

    // --- copy_dir_contents rejects symlink entries (#46 #2) ---

    #[cfg(unix)]
    #[test]
    fn copy_dir_contents_rejects_symlink_entry() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("lib");
        std::fs::create_dir(&src).unwrap();
        // A symlink pointing outside the source tree, as a crafted archive might.
        std::os::unix::fs::symlink("/etc/passwd", src.join("evil")).unwrap();
        let dst = dir.path().join("out");
        std::fs::create_dir(&dst).unwrap();

        assert!(matches!(
            copy_dir_contents(&src, &dst),
            Err(ExtractRunnerError::UnsafeArchiveEntry { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_contents_rejects_symlinked_root() {
        // A symlinked `lib` root would otherwise be followed by read_dir,
        // copying files from outside the extracted tree (#46 #2).
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"x").unwrap();
        let src = dir.path().join("lib");
        std::os::unix::fs::symlink(&outside, &src).unwrap();
        let dst = dir.path().join("out");
        std::fs::create_dir(&dst).unwrap();

        assert!(matches!(
            copy_dir_contents(&src, &dst),
            Err(ExtractRunnerError::UnsafeArchiveEntry { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn install_dir_atomic_swaps_existing_tree() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("newlib");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("a.olean"), b"new").unwrap();
        let dest = dir.path().join("lib/probe-lean-v4.15.0");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("stale.olean"), b"old").unwrap();

        install_dir_atomic(&src, &dest).unwrap();

        assert!(dest.join("a.olean").exists());
        assert!(!dest.join("stale.olean").exists());
        assert_eq!(std::fs::read(dest.join("a.olean")).unwrap(), b"new");
    }
}
