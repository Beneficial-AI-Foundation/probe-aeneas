use std::path::Path;
use std::process::Command;

/// Run `lake exe listfuns <output>` in the given Lean project directory.
pub fn run_listfuns(lean_project: &Path, output: &Path) -> Result<(), String> {
    let output_str = output
        .to_str()
        .ok_or_else(|| "Output path is not valid UTF-8".to_string())?;

    println!("Running `lake exe listfuns {output_str}` in {}...", lean_project.display());

    let status = Command::new("lake")
        .args(["exe", "listfuns", output_str])
        .current_dir(lean_project)
        .status()
        .map_err(|e| format!("Failed to run `lake exe listfuns`: {e}"))?;

    if !status.success() {
        return Err(format!(
            "`lake exe listfuns` exited with status {}",
            status.code().unwrap_or(-1)
        ));
    }

    if !output.exists() {
        return Err(format!(
            "`lake exe listfuns` completed but {} was not created",
            output.display()
        ));
    }

    println!("  Generated {}", output.display());
    Ok(())
}
