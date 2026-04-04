use std::path::{Path, PathBuf};
use std::process::Command;

use crate::extract::CharonConfig;
use crate::setup;

const PROBE_LEAN_GIT: &str = "https://github.com/Beneficial-AI-Foundation/probe-lean.git";

/// Run `probe-rust extract` on a project and return the path to the generated JSON.
///
/// When `output_dir` is provided, the output file is written there
/// (e.g. `.verilib/probes/`); otherwise a temp file is used.
pub fn run_probe_rust_extract(
    project: &Path,
    output_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let bin = find_or_install_probe_rust()?;
    let output = output_path(output_dir, "rust_extract", ".json");

    println!("Running probe-rust extract on {}...", project.display());
    let status = Command::new(&bin)
        .args([
            "extract",
            project
                .to_str()
                .ok_or_else(|| "Project path is not valid UTF-8".to_string())?,
            "-o",
            output
                .to_str()
                .ok_or_else(|| "Output path is not valid UTF-8".to_string())?,
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
///
/// When `output_dir` is provided, the output file is written there
/// (e.g. `.verilib/probes/`); otherwise a temp file is used.
pub fn run_probe_lean_extract(
    project: &Path,
    output_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    run_probe_lean_extract_with_opts(project, None, output_dir)
}

/// Run `probe-lean extract` with optional module prefix filter.
pub fn run_probe_lean_extract_with_opts(
    project: &Path,
    module_prefix: Option<&str>,
    output_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let bin = find_or_install_probe_lean(Some(project))?;
    let output = output_path(output_dir, "lean_extract", ".json");

    let project_str = project
        .to_str()
        .ok_or_else(|| "Project path is not valid UTF-8".to_string())?;
    let output_str = output
        .to_str()
        .ok_or_else(|| "Output path is not valid UTF-8".to_string())?;

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
    setup::find_or_install_probe_rust()
}

fn find_or_install_probe_lean(lean_project: Option<&Path>) -> Result<PathBuf, String> {
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
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create ~/.local/bin: {e}"))?;

    if version != "latest" {
        if let Ok(bin) = try_prebuilt_download(&version) {
            update_symlink(&bin)?;
            return Ok(bin);
        }
    }

    build_from_source(&version)
}

/// Read the Lean version from a project's `lean-toolchain` file.
fn detect_lean_version(project: &Path) -> Result<String, String> {
    let toolchain_path = project.join("lean-toolchain");
    let content = std::fs::read_to_string(&toolchain_path)
        .map_err(|e| format!("Failed to read lean-toolchain: {e}"))?;
    let trimmed = content.trim();
    let version = if let Some(after_colon) = trimmed.split(':').nth(1) {
        after_colon.trim().to_string()
    } else {
        trimmed.to_string()
    };
    if version.is_empty() {
        return Err("lean-toolchain file is empty".to_string());
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
fn try_prebuilt_download(lean_version: &str) -> Result<PathBuf, String> {
    let platform = detect_platform();
    let artifact = format!("probe-lean-{lean_version}-{platform}.tar.gz");
    println!("Checking for pre-built binary: {artifact}...");

    let output = Command::new("curl")
        .args([
            "-sL",
            "https://api.github.com/repos/Beneficial-AI-Foundation/probe-lean/releases",
        ])
        .output()
        .map_err(|e| format!("Failed to query GitHub releases: {e}"))?;

    if !output.status.success() {
        return Err("GitHub API request failed".to_string());
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

    let url = download_url
        .ok_or_else(|| "No pre-built binary available, falling back to source build".to_string())?;

    println!("Downloading pre-built binary...");

    let tmpdir = std::env::temp_dir().join("probe-lean-download");
    if tmpdir.exists() {
        std::fs::remove_dir_all(&tmpdir).ok();
    }
    std::fs::create_dir_all(&tmpdir).map_err(|e| format!("Failed to create temp dir: {e}"))?;

    let status = Command::new("bash")
        .args([
            "-c",
            &format!("curl -sL '{}' | tar -xz -C '{}'", url, tmpdir.display()),
        ])
        .status()
        .map_err(|e| format!("Failed to download/extract binary: {e}"))?;

    if !status.success() {
        return Err("Download/extraction failed".to_string());
    }

    let dest_dir = home_dir()?.join(".local/bin");
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create ~/.local/bin: {e}"))?;

    let versioned_bin = dest_dir.join(format!("probe-lean-{lean_version}"));
    let downloaded_bin = tmpdir.join("bin/probe-lean");
    if !downloaded_bin.exists() {
        return Err("Downloaded archive does not contain bin/probe-lean".to_string());
    }

    std::fs::copy(&downloaded_bin, &versioned_bin)
        .map_err(|e| format!("Failed to install binary: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&versioned_bin, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to set executable permission: {e}"))?;
    }

    let versioned_lib = home_dir()?.join(format!(".local/lib/probe-lean-{lean_version}"));
    let downloaded_lib = tmpdir.join("lib");
    if downloaded_lib.exists() {
        std::fs::create_dir_all(&versioned_lib)
            .map_err(|e| format!("Failed to create lib dir: {e}"))?;
        copy_dir_contents(&downloaded_lib, &versioned_lib)?;
    }

    std::fs::remove_dir_all(&tmpdir).ok();

    println!("  ✓ Installed pre-built probe-lean-{lean_version}");
    Ok(versioned_bin)
}

/// Build probe-lean from source for a specific Lean version.
fn build_from_source(lean_version: &str) -> Result<PathBuf, String> {
    println!("Building probe-lean from source for Lean {lean_version}...");

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

    if lean_version != "latest" {
        let toolchain_content = format!("leanprover/lean4:{lean_version}\n");
        std::fs::write(build_dir.join("lean-toolchain"), &toolchain_content)
            .map_err(|e| format!("Failed to write lean-toolchain: {e}"))?;

        let lake_manifest = build_dir.join("lake-manifest.json");
        if lake_manifest.exists() {
            std::fs::remove_file(&lake_manifest).ok();
        }
        let lake_dir = build_dir.join(".lake");
        if lake_dir.exists() {
            std::fs::remove_dir_all(&lake_dir).ok();
        }
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

    let (dest_bin, label) = if lean_version != "latest" {
        let versioned = dest_dir.join(format!("probe-lean-{lean_version}"));
        (versioned, format!("probe-lean-{lean_version}"))
    } else {
        (dest_dir.join("probe-lean"), "probe-lean".to_string())
    };

    std::fs::copy(&built_bin, &dest_bin)
        .map_err(|e| format!("Failed to copy probe-lean to ~/.local/bin: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest_bin, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to set executable permission: {e}"))?;
    }

    if lean_version != "latest" {
        update_symlink(&dest_bin)?;
    }

    println!("  ✓ Installed {label} to {}", dest_bin.display());
    Ok(dest_bin)
}

/// Update the `~/.local/bin/probe-lean` symlink to point at a versioned binary.
fn update_symlink(versioned_bin: &Path) -> Result<(), String> {
    let symlink = versioned_bin
        .parent()
        .ok_or("Invalid binary path")?
        .join("probe-lean");

    if symlink.exists() || symlink.symlink_metadata().is_ok() {
        std::fs::remove_file(&symlink).ok();
    }

    #[cfg(unix)]
    {
        let target = versioned_bin
            .file_name()
            .ok_or("Invalid versioned binary filename")?;
        std::os::unix::fs::symlink(target, &symlink)
            .map_err(|e| format!("Failed to create symlink: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::copy(versioned_bin, &symlink)
            .map_err(|e| format!("Failed to create probe-lean copy: {e}"))?;
    }
    Ok(())
}

/// Recursively copy directory contents from `src` to `dst`.
fn copy_dir_contents(src: &Path, dst: &Path) -> Result<(), String> {
    let entries =
        std::fs::read_dir(src).map_err(|e| format!("Failed to read dir {}: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read dir entry: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            std::fs::create_dir_all(&dst_path).map_err(|e| format!("Failed to create dir: {e}"))?;
            copy_dir_contents(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path).map_err(|e| format!("Failed to copy file: {e}"))?;
        }
    }
    Ok(())
}

/// Pre-generate the Charon LLBC file using config from `aeneas-config.yml`.
///
/// `probe-rust --with-charon` runs charon with only `--preset aeneas`, which
/// misses project-specific cargo args (e.g. `--no-default-features`),
/// `--start-from` filters, and `--exclude` lists. This function runs charon
/// directly with the full configuration so the LLBC is cached at
/// `<rust_project>/data/charon.llbc` before `probe-rust` needs it.
pub fn ensure_charon_llbc(rust_project: &Path, config: &CharonConfig) -> Result<(), String> {
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

    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("Failed to create {}: {e}", data_dir.display()))?;

    // Canonicalize to absolute path: charon resolves --dest-file relative to
    // its own cwd (rust_project), not probe-aeneas's cwd.
    let abs_llbc = std::fs::canonicalize(&data_dir)
        .map_err(|e| format!("Failed to canonicalize {}: {e}", data_dir.display()))?
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

    if let Some(ref start_from) = config.start_from {
        for item in start_from {
            args.push("--start-from".to_string());
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
        .map_err(|e| format!("Failed to run charon: {e}"))?;

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

fn find_on_path(name: &str) -> Option<PathBuf> {
    setup::find_on_path(name)
}

fn home_dir() -> Result<PathBuf, String> {
    setup::home_dir()
}

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
