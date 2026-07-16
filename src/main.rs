use clap::{Parser, Subcommand};
use probe_aeneas::{extract, extract_runner, listfuns, setup};
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
    /// The simplest invocation is a single project path:
    ///   probe-aeneas extract <project_path>
    ///
    /// This reads aeneas-config.yml from the project directory to auto-detect
    /// the Rust crate and Lean project locations. For advanced usage, provide
    /// explicit paths with --rust-project / --lean-project or pre-generated
    /// JSON files with --rust / --lean.
    Extract {
        /// Path to an Aeneas project directory (contains aeneas-config.yml).
        /// Auto-detects Rust and Lean project paths from the config.
        #[arg(
            value_name = "PROJECT",
            conflicts_with_all = ["rust", "rust_project", "lean", "lean_project"],
        )]
        project: Option<PathBuf>,

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
        /// Auto-generated from Lean sources when --lean-project or PROJECT is given.
        #[arg(long)]
        functions: Option<PathBuf>,

        /// Path to Aeneas's translation.json (emitted with the `emit-json` arg).
        /// Used as the authoritative loop/primary classification overlay.
        /// Auto-detected under `aeneas_args.dest` (default: project root) when
        /// PROJECT is given.
        #[arg(long)]
        translation: Option<PathBuf>,

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

        /// Run `cargo public-api` to compute accurate `is-public-api` on Rust atoms.
        /// Requires `cargo-public-api` installed and a nightly toolchain.
        #[arg(long)]
        with_public_api: bool,

        /// Skip the verification status enrichment step (transitive verification propagation)
        #[arg(long)]
        skip_enrich: bool,
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

        /// Path to Aeneas's translation.json (emitted with the `emit-json` arg).
        /// Optional authoritative loop/primary classification overlay.
        #[arg(long)]
        translation: Option<PathBuf>,

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

    /// Install external tool dependencies (probe-rust, charon).
    ///
    /// Installs probe-rust and charon, then delegates to `probe-rust setup`
    /// to install probe-rust's own dependencies (rust-analyzer, scip).
    /// Use --status to check which tools are installed without installing.
    /// probe-lean is installed automatically per-project during extract
    /// (version-matched to the target project's lean-toolchain).
    Setup {
        /// Show installation status instead of installing
        #[arg(long)]
        status: bool,
    },
}

#[allow(clippy::too_many_arguments)]
fn resolve_and_extract(
    project: Option<PathBuf>,
    rust: Option<PathBuf>,
    rust_project: Option<PathBuf>,
    lean: Option<PathBuf>,
    lean_project: Option<PathBuf>,
    functions: Option<PathBuf>,
    translation: Option<PathBuf>,
    output: Option<PathBuf>,
    aeneas_config: Option<PathBuf>,
    lake: bool,
    with_public_api: bool,
    skip_enrich: bool,
) -> anyhow::Result<()> {
    let (rust, rust_project, lean_project, functions, translation, rust_path_prefix, charon_config) =
        if let Some(ref proj) = project {
            let resolved = extract::resolve_project(proj)?;
            let prefix = if resolved.crate_dir != "." {
                Some(resolved.crate_dir.clone())
            } else {
                None
            };
            (
                None,
                Some(resolved.rust_project),
                Some(resolved.lean_project),
                functions.or(resolved.functions_json),
                translation.or(resolved.translation_json),
                prefix,
                resolved.charon_config,
            )
        } else {
            (
                rust,
                rust_project,
                lean_project,
                functions,
                translation,
                None,
                None,
            )
        };

    // Pre-flight: generate the charon LLBC with aeneas-config.yml args — but
    // ONLY when there is no Aeneas manifest. With a `translation.json`, probe-rust
    // reads charon `def_id`s from it (charon already ran once inside Aeneas), so
    // a second charon run is pure waste. Skipping it is the point of the manifest
    // path; the LLBC pre-flight remains for legacy (no-manifest) projects.
    if translation.is_none() {
        if let (Some(ref rp), Some(ref cc)) = (&rust_project, &charon_config) {
            extract_runner::ensure_charon_llbc(rp, cc, None)?;
        }
    }

    // Resolve the Aeneas build's active cargo feature set so cfg predicates on
    // Rust atoms can be evaluated for scope (KB P25). `None` when unresolvable —
    // cfg-based scope classification is then skipped (conservative).
    let cfg_config = rust_project
        .as_deref()
        .and_then(|rp| extract::resolve_active_features(rp, charon_config.as_ref()));

    extract::run_extract(
        rust.as_deref(),
        rust_project.as_deref(),
        lean.as_deref(),
        lean_project.as_deref(),
        functions.as_deref(),
        translation.as_deref(),
        output.as_deref(),
        aeneas_config.as_deref(),
        lake,
        rust_path_prefix.as_deref(),
        with_public_api,
        skip_enrich,
        cfg_config.as_ref(),
    )
    .map_err(anyhow::Error::new)
}

fn main() {
    let cli = Cli::parse();

    let result: anyhow::Result<()> = match cli.command {
        Commands::Extract {
            project,
            rust,
            rust_project,
            lean,
            lean_project,
            functions,
            translation,
            output,
            aeneas_config,
            lake,
            with_public_api,
            skip_enrich,
        } => resolve_and_extract(
            project,
            rust,
            rust_project,
            lean,
            lean_project,
            functions,
            translation,
            output,
            aeneas_config,
            lake,
            with_public_api,
            skip_enrich,
        ),

        Commands::Translate {
            rust,
            lean,
            functions,
            translation,
            output,
        } => extract::run_translate_only(&rust, &lean, &functions, translation.as_deref(), &output)
            .map_err(anyhow::Error::new),

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
                listfuns::run_listfuns(&lean_project, &output).map_err(anyhow::Error::new)
            } else if no_enrich {
                listfuns::run_basic_listfuns(&lean_project, &output).map_err(anyhow::Error::new)
            } else {
                listfuns::run_enriched_listfuns(
                    &lean_project,
                    &output,
                    atoms.as_deref(),
                    module_prefix.as_deref(),
                    aeneas_config.as_deref(),
                )
                .map_err(anyhow::Error::new)
            }
        }

        Commands::Setup { status } => {
            setup::cmd_setup(status);
            Ok(())
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
