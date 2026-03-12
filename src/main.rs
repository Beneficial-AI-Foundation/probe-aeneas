mod listfuns;
mod merge;
mod translate;
mod types;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "probe-aeneas")]
#[command(about = "Cross-language merge tool for Aeneas-transpiled projects (Rust ↔ Lean)")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Full pipeline: generate functions.json, build translations, merge atoms.
    Merge {
        /// Path to Rust atoms JSON (from probe-rust extract).
        #[arg(long)]
        rust: PathBuf,

        /// Path to Lean atoms JSON (from probe-lean extract).
        #[arg(long)]
        lean: PathBuf,

        /// Path to the Lean project directory (where `lake exe listfuns` runs).
        #[arg(long)]
        lean_project: PathBuf,

        /// Output path for the merged atoms JSON.
        #[arg(short, long, default_value = "merged_atoms.json")]
        output: PathBuf,
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
        Commands::Merge {
            rust,
            lean,
            lean_project,
            output,
        } => merge::run_merge(&rust, &lean, &lean_project, &output),

        Commands::Translate {
            rust,
            lean,
            functions,
            output,
        } => merge::run_translate_only(&rust, &lean, &functions, &output),

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
