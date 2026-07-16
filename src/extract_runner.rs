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

use anyhow::Context as _;

use crate::extract::CharonConfig;
use crate::setup;

const PROBE_LEAN_GIT: &str = "https://github.com/Beneficial-AI-Foundation/probe-lean.git";

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

    /// No pre-built binary available for the requested platform/version.
    /// Callers should fall back to building from source.
    #[error("No pre-built binary available, falling back to source build")]
    NoPrebuiltAvailable,

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
    let lean_version = lean_project.and_then(|p| detect_lean_version(p).ok());

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
fn detect_lean_version(project: &Path) -> Result<String> {
    let toolchain_path = project.join("lean-toolchain");
    let content = std::fs::read_to_string(&toolchain_path)
        .with_context(|| format!("read lean-toolchain at {}", toolchain_path.display()))?;
    let trimmed = content.trim();
    let version = if let Some(after_colon) = trimmed.split(':').nth(1) {
        after_colon.trim().to_string()
    } else {
        trimmed.to_string()
    };
    if version.is_empty() {
        return Err(ExtractRunnerError::LeanToolchainEmpty);
    }
    Ok(version)
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
    let download_url = body
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.contains("browser_download_url") && line.contains(&artifact) {
                line.split('"')
                    .find(|s| s.starts_with("https://") && s.contains(&artifact))
                    .map(String::from)
            } else {
                None
            }
        })
        .next();

    let url = download_url.ok_or(ExtractRunnerError::NoPrebuiltAvailable)?;

    println!("Downloading pre-built binary...");

    let tmpdir = std::env::temp_dir().join("probe-lean-download");
    if tmpdir.exists() {
        std::fs::remove_dir_all(&tmpdir).ok();
    }
    std::fs::create_dir_all(&tmpdir)
        .with_context(|| format!("create temp dir {}", tmpdir.display()))?;

    let status = Command::new("bash")
        .args([
            "-c",
            &format!("curl -sL '{}' | tar -xz -C '{}'", url, tmpdir.display()),
        ])
        .status()
        .context("spawn curl|tar pipeline to download probe-lean")?;

    if !status.success() {
        return Err(anyhow::anyhow!("Download/extraction failed").into());
    }

    let dest_dir = home_dir()?.join(".local/bin");
    std::fs::create_dir_all(&dest_dir).context("create ~/.local/bin")?;

    let versioned_bin = dest_dir.join(format!("probe-lean-{lean_version}"));
    let downloaded_bin = tmpdir.join("bin/probe-lean");
    if !downloaded_bin.exists() {
        return Err(anyhow::anyhow!("Downloaded archive does not contain bin/probe-lean").into());
    }

    std::fs::copy(&downloaded_bin, &versioned_bin).with_context(|| {
        format!(
            "copy {} to {}",
            downloaded_bin.display(),
            versioned_bin.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&versioned_bin, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("set +x on {}", versioned_bin.display()))?;
    }

    let versioned_lib = home_dir()?.join(format!(".local/lib/probe-lean-{lean_version}"));
    let downloaded_lib = tmpdir.join("lib");
    if downloaded_lib.exists() {
        std::fs::create_dir_all(&versioned_lib)
            .with_context(|| format!("create lib dir {}", versioned_lib.display()))?;
        copy_dir_contents(&downloaded_lib, &versioned_lib)?;
    }

    std::fs::remove_dir_all(&tmpdir).ok();

    println!("  ✓ Installed pre-built probe-lean-{lean_version}");
    Ok(versioned_bin)
}

/// Build probe-lean from source for a specific Lean version.
fn build_from_source(lean_version: &str) -> Result<PathBuf> {
    println!("Building probe-lean from source for Lean {lean_version}...");

    let build_dir = std::env::temp_dir().join("probe-lean-build");
    if build_dir.exists() {
        std::fs::remove_dir_all(&build_dir)
            .with_context(|| format!("clean build dir {}", build_dir.display()))?;
    }

    let status = Command::new("git")
        .args(["clone", "--depth", "1", PROBE_LEAN_GIT])
        .arg(&build_dir)
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
        .current_dir(&build_dir)
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

    let (dest_bin, label) = if lean_version != "latest" {
        let versioned = dest_dir.join(format!("probe-lean-{lean_version}"));
        (versioned, format!("probe-lean-{lean_version}"))
    } else {
        (dest_dir.join("probe-lean"), "probe-lean".to_string())
    };

    std::fs::copy(&built_bin, &dest_bin)
        .with_context(|| format!("copy {} to {}", built_bin.display(), dest_bin.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest_bin, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("set +x on {}", dest_bin.display()))?;
    }

    if lean_version != "latest" {
        update_symlink(&dest_bin)?;
    }

    println!("  ✓ Installed {label} to {}", dest_bin.display());
    Ok(dest_bin)
}

/// Update the `~/.local/bin/probe-lean` symlink to point at a versioned binary.
fn update_symlink(versioned_bin: &Path) -> Result<()> {
    let symlink = versioned_bin
        .parent()
        .context("versioned binary has no parent directory")?
        .join("probe-lean");

    if symlink.exists() || symlink.symlink_metadata().is_ok() {
        std::fs::remove_file(&symlink).ok();
    }

    #[cfg(unix)]
    {
        let target = versioned_bin
            .file_name()
            .context("versioned binary has no filename")?;
        std::os::unix::fs::symlink(target, &symlink)
            .with_context(|| format!("create symlink at {}", symlink.display()))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::copy(versioned_bin, &symlink)
            .with_context(|| format!("copy probe-lean to {}", symlink.display()))?;
    }
    Ok(())
}

/// Recursively copy directory contents from `src` to `dst`.
fn copy_dir_contents(src: &Path, dst: &Path) -> Result<()> {
    let entries = std::fs::read_dir(src).with_context(|| format!("read dir {}", src.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("read entry in {}", src.display()))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
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

/// Bytes of the LLBC prefix scanned for `charon_version`. Charon serializes the
/// field near the front of the JSON, so a bounded read avoids parsing the
/// multi-megabyte AST while tolerating some leading whitespace/formatting.
const LLBC_PREFIX_SCAN_BYTES: u64 = 64 * 1024;

/// Read the `charon_version` from a `.llbc` file cheaply.
///
/// Charon serializes `charon_version` near the front of the LLBC JSON, so a
/// bounded prefix read avoids parsing the multi-megabyte AST. Uses `read_to_end`
/// on a capped [`std::io::Take`] rather than a single `read` (which is allowed a
/// short read even on a regular file, and could miss the field). Tolerates
/// optional whitespace around the `:` so a pretty-printed LLBC still parses.
/// Returns `None` when the field is not found in the prefix (unexpected format),
/// on IO error, or when the value is empty (an empty version cannot gate).
fn read_llbc_charon_version(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    file.take(LLBC_PREFIX_SCAN_BYTES)
        .read_to_end(&mut buf)
        .ok()?;
    let prefix = String::from_utf8_lossy(&buf);
    parse_charon_version_prefix(&prefix)
}

/// Extract the `charon_version` value from a JSON prefix. Split out from IO so
/// the whitespace/format tolerance is unit-testable. Returns `None` when the key
/// is absent or its value is empty.
fn parse_charon_version_prefix(prefix: &str) -> Option<String> {
    let key = "\"charon_version\"";
    let after_key = &prefix[prefix.find(key)? + key.len()..];
    // Skip whitespace, then the `:`, then whitespace, then the opening quote.
    let rest = after_key.trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let value = rest.strip_prefix('"')?;
    let end = value.find('"')?;
    let version = &value[..end];
    (!version.is_empty()).then(|| version.to_string())
}

/// Decide whether a cached LLBC must be regenerated to keep the charon-def-id
/// join sound. `cached` is the version parsed from the cache; `expected` is
/// Aeneas's `translation.json` `charon_version`.
///
/// Fails **closed**: when an expected version is known but the cache's version
/// is unreadable (`None`, e.g. a format change or truncated file), the cache is
/// treated as stale and regenerated rather than silently trusted. Only when
/// there is nothing to compare against (`expected` is `None`) or both agree is
/// the cache kept.
fn llbc_cache_is_stale(cached: Option<&str>, expected: Option<&str>) -> bool {
    match (cached, expected) {
        // Nothing to compare against: keep the cache (join stays name-based).
        (_, None) => false,
        // Known-good match: keep.
        (Some(c), Some(e)) if c == e => false,
        // Mismatch, or an expected version with an unreadable cache: regenerate.
        (_, Some(_)) => true,
    }
}

/// Pre-generate the Charon LLBC file using config from `aeneas-config.yml`.
///
/// `probe-rust --with-charon` runs charon with only `--preset aeneas`, which
/// misses project-specific cargo args (e.g. `--no-default-features`),
/// `--start-from` filters, and `--exclude` lists. This function runs charon
/// directly with the full configuration so the LLBC is cached at
/// `<rust_project>/data/charon.llbc` before `probe-rust` needs it.
///
/// `expected_charon_version` is Aeneas's `translation.json` `charon_version`.
/// A cached LLBC produced by a *different* charon run yields `charon-def-id`s
/// that point at different functions than the manifest's `def_id`s, so it must
/// not feed the id-join. On mismatch the stale cache is discarded and
/// regenerated; if regeneration still cannot match (the installed charon
/// differs from Aeneas's), a warning notes that the id-join will be disabled by
/// its provenance gate and matching falls back to names.
pub fn ensure_charon_llbc(
    rust_project: &Path,
    config: &CharonConfig,
    expected_charon_version: Option<&str>,
) -> Result<()> {
    let data_dir = rust_project.join("data");
    let llbc_path = data_dir.join("charon.llbc");

    if llbc_path.exists() {
        let cached = read_llbc_charon_version(&llbc_path);
        if llbc_cache_is_stale(cached.as_deref(), expected_charon_version) {
            eprintln!(
                "  ⚠ Cached Charon LLBC is charon {} but translation.json is charon {}; \
                 regenerating to keep the charon-def-id join sound.",
                cached.as_deref().unwrap_or("unreadable"),
                expected_charon_version.unwrap_or("?"),
            );
            // Ignore removal errors: charon overwrites --dest-file below, so a
            // failed unlink (e.g. ENOENT race) still yields a fresh LLBC.
            let _ = std::fs::remove_file(&llbc_path);
        } else {
            println!("Using cached Charon LLBC at {}", llbc_path.display());
            return Ok(());
        }
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

    // Post-generation provenance check: if the installed charon still cannot
    // match the manifest, the id-join will be gated off downstream — say so.
    if let (Some(expected), Some(actual)) = (
        expected_charon_version,
        read_llbc_charon_version(&llbc_path),
    ) {
        if actual != expected {
            eprintln!(
                "  ⚠ Regenerated Charon LLBC is charon {actual}, but translation.json is \
                 charon {expected}. The charon-def-id join will be disabled (provenance gate); \
                 matching falls back to names. Install charon {expected} to enable the join.",
            );
        }
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

    fn write_tmp(contents: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("charon.llbc");
        std::fs::write(&path, contents).unwrap();
        (dir, path)
    }

    #[test]
    fn read_llbc_charon_version_extracts_first_field() {
        // Realistic LLBC prefix: charon_version is the first key.
        let (_d, path) =
            write_tmp(br#"{"charon_version":"0.1.217","translated":{"crate_name":"spqr"}}"#);
        assert_eq!(read_llbc_charon_version(&path).as_deref(), Some("0.1.217"));
    }

    #[test]
    fn read_llbc_charon_version_handles_large_prefix() {
        // Version present, followed by a body larger than the read buffer.
        let mut bytes = br#"{"charon_version":"0.1.174","translated":"#.to_vec();
        bytes.extend(std::iter::repeat_n(b'x', 8192));
        let (_d, path) = write_tmp(&bytes);
        assert_eq!(read_llbc_charon_version(&path).as_deref(), Some("0.1.174"));
    }

    #[test]
    fn read_llbc_charon_version_none_when_absent() {
        let (_d, path) = write_tmp(br#"{"translated":{"crate_name":"spqr"}}"#);
        assert_eq!(read_llbc_charon_version(&path), None);
    }

    #[test]
    fn read_llbc_charon_version_none_on_missing_file() {
        assert_eq!(
            read_llbc_charon_version(Path::new("/nonexistent/charon.llbc")),
            None
        );
    }

    #[test]
    fn read_llbc_charon_version_tolerates_whitespace() {
        // A pretty-printed LLBC inserts spaces/newlines around the `:`.
        let (_d, path) =
            write_tmp(b"{\n  \"charon_version\" : \"0.1.217\",\n  \"translated\": {}\n}");
        assert_eq!(read_llbc_charon_version(&path).as_deref(), Some("0.1.217"));
    }

    #[test]
    fn read_llbc_charon_version_none_on_empty_value() {
        // An empty version cannot gate anything, so it reads as absent.
        let (_d, path) = write_tmp(br#"{"charon_version":"","translated":{}}"#);
        assert_eq!(read_llbc_charon_version(&path), None);
    }

    #[test]
    fn parse_charon_version_prefix_handles_formats() {
        assert_eq!(
            parse_charon_version_prefix(r#"{"charon_version":"1.2.3"}"#).as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            parse_charon_version_prefix(r#"{ "charon_version" :  "1.2.3" }"#).as_deref(),
            Some("1.2.3")
        );
        assert_eq!(parse_charon_version_prefix(r#"{"other":"x"}"#), None);
        assert_eq!(
            parse_charon_version_prefix(r#"{"charon_version":""}"#),
            None
        );
        // Non-string value: no opening quote after the colon -> None.
        assert_eq!(
            parse_charon_version_prefix(r#"{"charon_version":123}"#),
            None
        );
    }

    #[test]
    fn llbc_cache_is_stale_decision_matrix() {
        // Nothing to compare against: keep the cache regardless of what it holds.
        assert!(!llbc_cache_is_stale(None, None));
        assert!(!llbc_cache_is_stale(Some("0.1.217"), None));
        // Known-good match: keep.
        assert!(!llbc_cache_is_stale(Some("0.1.217"), Some("0.1.217")));
        // Version mismatch: regenerate.
        assert!(llbc_cache_is_stale(Some("0.1.174"), Some("0.1.217")));
        // Expected version known but cache unreadable: fail closed -> regenerate.
        assert!(llbc_cache_is_stale(None, Some("0.1.217")));
    }
}
