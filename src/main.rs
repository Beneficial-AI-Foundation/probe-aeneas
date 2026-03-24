use clap::{Parser, Subcommand};
use probe_aeneas::{extract, gen_functions, listfuns};
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

        /// Use `lake exe listfuns` to generate functions.json instead of
        /// parsing Aeneas-generated Lean files directly. Requires the Lean
        /// project to define a `listfuns` executable.
        #[arg(long)]
        lake: bool,
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

    /// Generate functions.json from a Lean project.
    ///
    /// By default, parses Aeneas-generated `.lean` files directly and enriches
    /// with verification data from probe-lean. Use --no-enrich for a basic
    /// function list without verification data. Use --lake to delegate to the
    /// project's own `lake exe listfuns` executable.
    Listfuns {
        /// Path to the Lean project directory.
        #[arg(long)]
        lean_project: PathBuf,

        /// Output path for functions.json.
        #[arg(short, long, default_value = "functions.json")]
        output: PathBuf,

        /// Use `lake exe listfuns` instead of parsing Lean files directly.
        #[arg(long)]
        lake: bool,

        /// Skip enrichment (no probe-lean call, basic function list only).
        #[arg(long)]
        no_enrich: bool,

        /// Path to pre-computed atoms JSON (from probe-lean extract).
        /// Skips the internal probe-lean invocation when provided.
        #[arg(long)]
        atoms: Option<PathBuf>,

        /// Module prefix filter passed to probe-lean extract via -m.
        /// Optional optimization to limit atom extraction scope.
        #[arg(long, name = "module")]
        module_prefix: Option<String>,

        /// Path to Aeneas config JSON for manual overrides (is-hidden).
        /// Defaults to .verilib/aeneas.json in the Lean project directory.
        #[arg(long)]
        aeneas_config: Option<PathBuf>,
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
            lake,
        } => extract::run_extract(
            rust.as_deref(),
            rust_project.as_deref(),
            lean.as_deref(),
            lean_project.as_deref(),
            functions.as_deref(),
            output.as_deref(),
            aeneas_config.as_deref(),
            lake,
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
            lake,
            no_enrich,
            atoms,
            module_prefix,
            aeneas_config,
        } => {
            if lake {
                listfuns::run_listfuns(&lean_project, &output)
            } else if no_enrich {
                gen_functions::generate_functions_json(&lean_project, &output)
            } else {
                listfuns::run_enriched_listfuns(
                    &lean_project,
                    &output,
                    atoms.as_deref(),
                    module_prefix.as_deref(),
                    aeneas_config.as_deref(),
                )
            }
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
