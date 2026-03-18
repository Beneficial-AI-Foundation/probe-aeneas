mod aeneas_config;
mod extract;
mod extract_runner;
mod listfuns;
mod translate;
mod types;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "probe-aeneas")]
#[command(about = "Cross-language extract tool for Aeneas-transpiled projects (Rust ↔ Lean)")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Full pipeline: extract atoms (if needed), generate translations, and
    /// merge Rust + Lean call graphs into a unified atom file.
    ///
    /// Provide either pre-generated JSON files (--rust / --lean) or project paths
    /// (--rust-project / --lean-project) which will run probe-rust/probe-lean
    /// automatically.
    Extract {
        /// Path to pre-generated Rust atoms JSON (from probe-rust extract).
        #[arg(long, group = "rust_input")]
        rust: Option<PathBuf>,

        /// Path to a Rust project directory (runs probe-rust extract automatically).
        #[arg(long, group = "rust_input")]
        rust_project: Option<PathBuf>,

        /// Path to pre-generated Lean atoms JSON (from probe-lean extract).
        /// Can be combined with --lean-project to use pre-computed atoms
        /// while auto-generating functions.json from the project directory.
        #[arg(long)]
        lean: Option<PathBuf>,

        /// Path to a Lean project directory (runs probe-lean extract automatically,
        /// or provides functions.json generation when combined with --lean).
        #[arg(long)]
        lean_project: Option<PathBuf>,

        /// Path to functions.json (Aeneas name mapping).
        /// Auto-generated via `lake exe listfuns` when --lean-project is given.
        #[arg(long)]
        functions: Option<PathBuf>,

        /// Output path for the merged atoms JSON.
        /// Defaults to aeneas_{package}_{version}.json based on the Rust input.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Path to Aeneas config JSON for manual overrides (is-hidden, is-ignored).
        /// Defaults to .verilib/aeneas.json in the Lean project directory.
        #[arg(long)]
        aeneas_config: Option<PathBuf>,
    },

    /// Generate a translations file mapping Rust code-names to Lean code-names.
    Translate {
        /// Path to Rust atoms JSON (from probe-rust extract).
        #[arg(long)]
        rust: PathBuf,

        /// Path to Lean atoms JSON (from probe-lean extract).
        #[arg(long)]
        lean: PathBuf,

        /// Path to functions.json (from `lake exe listfuns`).
        #[arg(long)]
        functions: PathBuf,

        /// Output path for the translations JSON.
        #[arg(short, long, default_value = "translations.json")]
        output: PathBuf,
    },

    /// Generate functions.json by running `lake exe listfuns` in a Lean project.
    Listfuns {
        /// Path to the Lean project directory.
        #[arg(long)]
        lean_project: PathBuf,

        /// Output path for functions.json.
        #[arg(short, long, default_value = "functions.json")]
        output: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Extract {
            rust,
            rust_project,
            lean,
            lean_project,
            functions,
            output,
            aeneas_config,
        } => extract::run_extract(
            rust.as_deref(),
            rust_project.as_deref(),
            lean.as_deref(),
            lean_project.as_deref(),
            functions.as_deref(),
            output.as_deref(),
            aeneas_config.as_deref(),
        ),

        Commands::Translate {
            rust,
            lean,
            functions,
            output,
        } => extract::run_translate_only(&rust, &lean, &functions, &output),

        Commands::Listfuns {
            lean_project,
            output,
        } => listfuns::run_listfuns(&lean_project, &output),
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
