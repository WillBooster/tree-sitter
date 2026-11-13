// code-gauge-allow-file fileNcss 608 -- clap subcommand definitions and CLI dispatch live in main.rs
use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

use anstyle::{AnsiColor, Color, Style};
use anyhow::{Context, Result, anyhow};
use clap::{ArgGroup, Args, Command, FromArgMatches as _, Subcommand, ValueEnum, crate_authors};
use clap_complete::generate;
use dialoguer::{Confirm, FuzzySelect, Input, MultiSelect, theme::ColorfulTheme};
use heck::ToUpperCamelCase;
use log::{error, info, warn};
use regex::Regex;
use semver::Version as SemverVersion;
use tree_sitter::{Parser, ffi};
use tree_sitter_cli::{
    fuzz::{
        DEFAULT_EDIT_COUNT, DEFAULT_ITERATION_COUNT, EDIT_COUNT, FuzzOptions, ITERATION_COUNT,
        LOG_ENABLED, LOG_GRAPH_ENABLED, START_SEED, fuzz_language_corpus,
    },
    init::{JsonConfigOpts, TREE_SITTER_JSON_SCHEMA, generate_grammar_files},
    input::{CliInput, get_input, get_tmp_source_file},
    logger, paint,
    parse::{self, ParseDebugType, ParseFileOptions, ParseOutput, ParseTheme},
    playground,
    test::{self, TestOptions, TestStats, TestSummary},
    util,
    version::{self, BumpLevel},
    wasm,
};
use tree_sitter_config::Config;
use tree_sitter_generate::{Diagnostic, GenerateError, OptLevel};
use tree_sitter_loader::{self as loader, Bindings, TreeSitterJSON};

const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD_SHA: Option<&'static str> = option_env!("BUILD_SHA");

#[derive(Subcommand)]
#[command(about="Generates and tests parsers", author=crate_authors!("\n"), styles=get_styles())]
enum Commands {
    /// Generate a default config file
    InitConfig(InitConfig),
    /// Initialize a grammar repository
    Init(Init),
    /// Generate a parser
    Generate(Generate),
    /// Record a generation profile from an unprofiled parser
    Profile(Profile),
    /// Compile a parser
    Build(Build),
    /// Parse files
    Parse(Parse),
    /// Run a parser's tests
    Test(Test),
    /// Display or increment the version of a grammar
    Version(Version),
    /// Fuzz a parser
    Fuzz(Fuzz),
    /// Start local playground for a parser in the browser
    Playground(Playground),
    /// Print info about all known language parsers
    DumpLanguages(DumpLanguages),
    /// Generate shell completions
    Complete(Complete),
}

#[derive(Args)]
struct InitConfig;

#[derive(Args)]
#[command(alias = "i")]
struct Init {
    /// Update outdated files
    #[arg(long, short)]
    pub update: bool,
    /// The path to the tree-sitter grammar directory
    #[arg(long, short = 'p')]
    pub grammar_path: Option<PathBuf>,
}

#[derive(Args)]
#[command(group(ArgGroup::new("training").required(true).multiple(true).args(["corpus", "paths"])))]
struct Profile {
    /// The path to the grammar directory containing the unprofiled parser
    #[arg(long, short = 'p')]
    grammar_path: Option<PathBuf>,
    /// Write the generation profile to this file, relative to the working directory
    #[arg(long, short)]
    output: PathBuf,
    /// Read a corpus file or directory, relative to the working directory (repeatable)
    #[arg(long)]
    corpus: Vec<PathBuf>,
    /// Match a parser generated with --disable-optimizations
    #[arg(long)]
    disable_optimizations: bool,
    /// Additional source files to parse; paths are relative to the working directory
    paths: Vec<PathBuf>,
}

#[derive(Args)]
#[command(alias = "gen", alias = "g")]
struct Generate {
    /// Apply state frequencies recorded from the same unprofiled parser
    #[arg(long, conflicts_with_all = ["no_parser", "report_states_for_rule"])]
    pub profile: Option<PathBuf>,
    /// The path to the grammar file
    #[arg(index = 1)]
    pub grammar_path: Option<PathBuf>,
    /// Show debug log during generation
    #[arg(long, short)]
    pub log: bool,
    /// Generate `grammar.json` and `node-types.json`, including parse-table validation, without C output
    #[arg(long)]
    pub no_parser: bool,
    /// Deprecated: use the `build` command
    #[arg(long, short = 'b')]
    pub build: bool,
    /// Deprecated: use the `build` command
    #[arg(long, short = '0')]
    pub debug_build: bool,
    /// Deprecated: use the `build` command
    #[arg(long, value_name = "PATH")]
    pub libdir: Option<PathBuf>,
    /// The path to output the generated source files
    #[arg(long, short, value_name = "DIRECTORY")]
    pub output: Option<PathBuf>,
    /// Produce a report of the states for the given rule, use `-` to report every rule
    #[arg(long, conflicts_with = "json", conflicts_with = "json_summary")]
    pub report_states_for_rule: Option<String>,
    /// Deprecated: use --json-summary
    #[arg(
        long,
        conflicts_with = "json_summary",
        conflicts_with = "report_states_for_rule"
    )]
    pub json: bool,
    /// Report conflicts in a JSON format
    #[arg(
        long,
        conflicts_with = "json",
        conflicts_with = "report_states_for_rule"
    )]
    pub json_summary: bool,
    /// The name or path of the JavaScript runtime to use for generating parsers
    #[cfg(not(feature = "qjs-rt"))]
    #[arg(
        long,
        value_name = "EXECUTABLE",
        env = "TREE_SITTER_JS_RUNTIME",
        default_value = "node"
    )]
    pub js_runtime: Option<String>,

    #[cfg(feature = "qjs-rt")]
    #[arg(
        long,
        value_name = "EXECUTABLE",
        env = "TREE_SITTER_JS_RUNTIME",
        default_value = "node"
    )]
    /// The name or path of the JavaScript runtime to use for generating parsers, specify `native`
    /// to use the native `QuickJS` runtime
    pub js_runtime: Option<String>,

    /// Disable optimizations when generating the parser. Currently, this only affects
    /// the merging of compatible parse states.
    #[arg(long)]
    pub disable_optimizations: bool,
}

#[derive(Args)]
#[command(alias = "b")]
struct Build {
    /// Compiler optimization: 2, 3, s (size), or z (minimum size); defaults to 2 natively and s for Wasm
    #[arg(long, conflicts_with = "debug", value_name = "LEVEL")]
    pub optimization: Option<loader::OptimizationLevel>,
    /// Build a Wasm module instead of a dynamic library
    #[arg(short, long)]
    pub wasm: bool,
    /// The path to output the compiled file
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// The path to the grammar directory
    #[arg(index = 1, num_args = 1)]
    pub path: Option<PathBuf>,
    /// Make the parser reuse the same allocator as the library
    #[arg(long)]
    pub reuse_allocator: bool,
    /// Compile a parser in debug mode
    #[arg(long, short = '0')]
    pub debug: bool,
    /// Display verbose build information
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Args)]
#[command(alias = "p")]
#[command(group(ArgGroup::new("graph_output").multiple(true)))]
struct Parse {
    /// The path to a file with paths to source file(s)
    #[arg(long = "paths")]
    pub paths_file: Option<PathBuf>,
    /// The source file(s) to use
    #[arg(num_args=1..)]
    pub paths: Option<Vec<PathBuf>>,
    /// The path to the tree-sitter grammar directory, implies --rebuild
    #[arg(long, short = 'p', conflicts_with = "rebuild")]
    pub grammar_path: Option<PathBuf>,
    /// The path to the parser's dynamic library
    #[arg(long, short = 'l')]
    pub lib_path: Option<PathBuf>,
    /// If `--lib-path` is used, the name of the language used to extract the
    /// library's language function
    #[arg(long, requires = "lib_path")]
    pub lang_name: Option<String>,
    /// Select a language by the scope instead of a file extension
    #[arg(long)]
    pub scope: Option<String>,
    /// Show parsing debug log
    #[arg(long, short = 'd')] // TODO: Rework once clap adds `default_missing_value_t`
    #[expect(
        clippy::option_option,
        reason = "required by clap for optional flag with optional value"
    )]
    pub debug: Option<Option<ParseDebugType>>,
    /// Compile a parser in debug mode
    #[arg(long, short = '0')]
    pub debug_build: bool,
    /// Produce the log.html file with debug graphs
    #[arg(long, short = 'D', group = "graph_output")]
    pub debug_graph: bool,
    /// Compile parsers to Wasm instead of native dynamic libraries
    #[arg(long, hide = cfg!(not(feature = "wasm")))]
    pub wasm: bool,
    /// Output the parse data with graphviz dot
    #[arg(long = "dot", group = "graph_output")]
    pub output_dot: bool,
    /// Output the parse data in XML format
    #[arg(long = "xml", short = 'x')]
    pub output_xml: bool,
    /// Output the parse data in a pretty-printed CST format
    #[arg(long = "cst", short = 'c')]
    pub output_cst: bool,
    /// Show parsing statistics
    #[arg(long, short, conflicts_with = "json", conflicts_with = "json_summary")]
    pub stat: bool,
    /// Interrupt the parsing process by timeout (µs)
    #[arg(long)]
    pub timeout: Option<u64>,
    /// Measure execution time
    #[arg(long, short)]
    pub time: bool,
    /// Suppress main output
    #[arg(long, short)]
    pub quiet: bool,
    /// The encoding of the input files
    #[arg(long)]
    pub encoding: Option<Encoding>,
    /// Open `log.html` in the default browser, if `--debug-graph` or `--dot` is supplied
    #[arg(long, requires = "graph_output")]
    pub open_log: bool,
    /// Deprecated: use --json-summary
    #[arg(long, conflicts_with = "json_summary", conflicts_with = "stat")]
    pub json: bool,
    /// Output parsing results in a JSON format
    #[arg(long, short = 'j', conflicts_with = "json", conflicts_with = "stat")]
    pub json_summary: bool,
    /// The path to an alternative config.json file
    #[arg(long)]
    pub config_path: Option<PathBuf>,
    /// Parse the contents of a specific test
    #[arg(long, short = 'n')]
    #[clap(conflicts_with = "paths", conflicts_with = "paths_file")]
    pub test_number: Option<u32>,
    /// Force rebuild the parser
    #[arg(short, long)]
    pub rebuild: bool,
    /// Omit ranges in the output
    #[arg(long)]
    pub no_ranges: bool,
}

#[derive(ValueEnum, Clone)]
pub enum Encoding {
    Utf8 = 0,
    Utf16LE = 1,
    Utf16BE = 2,
}

#[derive(Args)]
#[command(alias = "t")]
struct Test {
    /// Only run corpus test cases whose name matches the given regex
    #[arg(long, short)]
    pub include: Option<Regex>,
    /// Only run corpus test cases whose name does not match the given regex
    #[arg(long, short)]
    pub exclude: Option<Regex>,
    /// Only run corpus test cases from a given filename
    #[arg(long)]
    pub file_name: Option<String>,
    /// The path to the tree-sitter grammar directory, implies --rebuild
    #[arg(long, short = 'p', conflicts_with = "rebuild")]
    pub grammar_path: Option<PathBuf>,
    /// The path to the parser's dynamic library
    #[arg(long, short = 'l')]
    pub lib_path: Option<PathBuf>,
    /// If `--lib-path` is used, the name of the language used to extract the
    /// library's language function
    #[arg(long, requires = "lib_path")]
    pub lang_name: Option<String>,
    /// Update all syntax trees in corpus files with current parser output
    #[arg(long, short)]
    pub update: bool,
    /// Show parsing debug log
    #[arg(long, short = 'd')]
    pub debug: bool,
    /// Compile a parser in debug mode
    #[arg(long, short = '0')]
    pub debug_build: bool,
    /// Produce the log.html file with debug graphs
    #[arg(long, short = 'D')]
    pub debug_graph: bool,
    /// Compile parsers to Wasm instead of native dynamic libraries
    #[arg(long, hide = cfg!(not(feature = "wasm")))]
    pub wasm: bool,
    /// Open `log.html` in the default browser, if `--debug-graph` is supplied
    #[arg(long, requires = "debug_graph")]
    pub open_log: bool,
    /// Force showing fields in test diffs
    #[arg(long)]
    pub show_fields: bool,
    /// Force showing '+' and '-' in test diffs
    #[arg(long)]
    pub show_diff_markers: bool,
    /// Show parsing statistics
    #[arg(long)]
    pub stat: Option<TestStats>,
    /// Force rebuild the parser
    #[arg(short, long)]
    pub rebuild: bool,
    /// Show only the pass-fail overview tree
    #[arg(long)]
    pub overview_only: bool,
    /// Output the test summary in a JSON format
    #[arg(long)]
    pub json_summary: bool,
}

#[derive(Args)]
#[command(alias = "publish")]
#[expect(
    clippy::struct_field_names,
    reason = "field names map to CLI arguments"
)]
/// Display or increment the version of a grammar
struct Version {
    /// The version to bump to
    #[arg(
        conflicts_with = "bump",
        long_help = "\
        The version to bump to\n\
        \n\
        Examples:\n    \
            tree-sitter version: display the current version\n    \
            tree-sitter version <version>: bump to specified version\n    \
            tree-sitter version --bump <level>: automatic bump"
    )]
    pub version: Option<SemverVersion>,
    /// The path to the tree-sitter grammar directory
    #[arg(long, short = 'p')]
    pub grammar_path: Option<PathBuf>,
    /// Automatically bump from the current version
    #[arg(long, value_enum, conflicts_with = "version")]
    pub bump: Option<BumpLevel>,
}

#[derive(Args)]
#[command(alias = "f")]
struct Fuzz {
    /// List of test names to skip
    #[arg(long, short)]
    pub skip: Option<Vec<String>>,
    /// Subdirectory to the language
    #[arg(long)]
    pub subdir: Option<PathBuf>,
    /// The path to the tree-sitter grammar directory, implies --rebuild
    #[arg(long, short = 'p', conflicts_with = "rebuild")]
    pub grammar_path: Option<PathBuf>,
    /// The path to the parser's dynamic library
    #[arg(long)]
    pub lib_path: Option<PathBuf>,
    /// If `--lib-path` is used, the name of the language used to extract the
    /// library's language function
    #[arg(long, requires = "lib_path")]
    pub lang_name: Option<String>,
    #[arg(
        long,
        help=format!("Maximum number of edits to perform per fuzz test (Default: {DEFAULT_EDIT_COUNT})")
    )]
    pub edits: Option<usize>,
    #[arg(
        long,
        help=format!("Number of fuzzing iterations to run per test (Default: {DEFAULT_ITERATION_COUNT})")
    )]
    pub iterations: Option<usize>,
    /// Only fuzz corpus test cases whose name matches the given regex
    #[arg(long, short)]
    pub include: Option<Regex>,
    /// Only fuzz corpus test cases whose name does not match the given regex
    #[arg(long, short)]
    pub exclude: Option<Regex>,
    /// Enable logging of graphs and input
    #[arg(long)]
    pub log_graphs: bool,
    /// Enable parser logging
    #[arg(long, short)]
    pub log: bool,
    /// Force rebuild the parser
    #[arg(short, long)]
    pub rebuild: bool,
}

#[derive(Args)]
#[command(alias = "play", alias = "pg", alias = "web-ui")]
struct Playground {
    /// Don't open in default browser
    #[arg(long, short)]
    pub quiet: bool,
    /// Path to the directory containing the grammar and Wasm files
    #[arg(long)]
    pub grammar_path: Option<PathBuf>,
    /// Export playground files to specified directory instead of serving them
    #[arg(long, short)]
    pub export: Option<PathBuf>,
}

#[derive(Args)]
#[command(alias = "langs")]
struct DumpLanguages {
    /// The path to an alternative config.json file
    #[arg(long)]
    pub config_path: Option<PathBuf>,
}

#[derive(Args)]
#[command(alias = "comp")]
struct Complete {
    /// The shell to generate completions for
    #[arg(long, short, value_enum)]
    pub shell: Shell,
}

#[derive(ValueEnum, Clone)]
pub enum Shell {
    Bash,
    Elvish,
    Fish,
    PowerShell,
    Zsh,
    Nushell,
}

/// Complete `action` if the wasm feature is enabled, otherwise return an error
macro_rules! checked_wasm {
    ($action:block) => {
        #[cfg(feature = "wasm")]
        {
            $action
        }
        #[cfg(not(feature = "wasm"))]
        {
            Err(anyhow!("--wasm flag specified, but this build of tree-sitter-cli does not include the wasm feature"))?;
        }
    };
}

impl InitConfig {
    fn run() -> Result<()> {
        if let Ok(Some(config_path)) = Config::find_config_file() {
            return Err(anyhow!(
                "Remove your existing config file first: {}",
                config_path.to_string_lossy()
            ));
        }
        let mut config = Config::initial()?;
        config.add(tree_sitter_loader::Config::initial())?;
        config.save()?;
        info!(
            "Saved initial configuration to {}",
            config.location.display()
        );
        Ok(())
    }
}

impl Init {
    fn run(self, current_dir: &Path) -> Result<()> {
        let configure_json = !current_dir.join("tree-sitter.json").exists();

        let (language_name, json_config_opts) = if configure_json {
            let mut opts = JsonConfigOpts::default();

            let name = || {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("Parser name")
                    .validate_with(|input: &String| {
                        if input.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                            Ok(())
                        } else {
                            Err("The name must be lowercase and contain only letters, digits, and underscores")
                        }
                    })
                    .interact_text()
            };

            let camelcase_name = |name: &str| {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("CamelCase name")
                    .default(name.to_upper_camel_case())
                    .validate_with(|input: &String| {
                        if input
                            .chars()
                            .all(|c| c.is_ascii_alphabetic() || c.is_ascii_digit() || c == '_')
                        {
                            Ok(())
                        } else {
                            Err("The name must contain only letters, digits, and underscores")
                        }
                    })
                    .interact_text()
            };

            let title = |name: &str| {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("Title (human-readable name)")
                    .default(name.to_upper_camel_case())
                    .interact_text()
            };

            let description = |name: &str| {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("Description")
                    .default(format!(
                        "{} grammar for tree-sitter",
                        name.to_upper_camel_case()
                    ))
                    .show_default(false)
                    .allow_empty(true)
                    .interact_text()
            };

            let repository = |name: &str| {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("Repository URL")
                    .allow_empty(true)
                    .default(format!("https://github.com/tree-sitter/tree-sitter-{name}"))
                    .show_default(false)
                    .interact_text()
            };

            let funding = || {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("Funding URL")
                    .allow_empty(true)
                    .interact_text()
                    .map(|e| Some(e.trim().to_string()))
            };

            let scope = |name: &str| {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("TextMate scope")
                    .default(format!("source.{name}"))
                    .validate_with(|input: &String| {
                        if input.starts_with("source.") || input.starts_with("text.") {
                            Ok(())
                        } else {
                            Err("The scope must start with 'source.' or 'text.'")
                        }
                    })
                    .interact_text()
            };

            let file_types = |name: &str| {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("File types (space-separated)")
                    .default(name.to_string())
                    .interact_text()
                    .map(|ft| {
                        let mut set = HashSet::new();
                        for ext in ft.split(' ') {
                            let ext = ext.trim();
                            if !ext.is_empty() {
                                set.insert(ext.to_string());
                            }
                        }
                        set.into_iter().collect::<Vec<_>>()
                    })
            };

            let initial_version = || {
                Input::<SemverVersion>::with_theme(&ColorfulTheme::default())
                    .with_prompt("Version")
                    .default(SemverVersion::new(0, 1, 0))
                    .interact_text()
            };

            let license = || {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("License")
                    .default("MIT".to_string())
                    .allow_empty(true)
                    .interact()
            };

            let author = || {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("Author name")
                    .interact_text()
            };

            let email = || {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("Author email")
                    .allow_empty(true)
                    .interact_text()
                    .map(|e| (!e.trim().is_empty()).then_some(e))
            };

            let url = || {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("Author URL")
                    .allow_empty(true)
                    .interact_text()
                    .map(|e| Some(e.trim().to_string()))
            };

            let namespace = || {
                Input::<String>::with_theme(&ColorfulTheme::default())
                    .with_prompt("Package namespace")
                    .default("io.github.tree-sitter".to_string())
                    .allow_empty(true)
                    .interact()
            };

            let bindings = || {
                let languages = Bindings::default().languages();

                let enabled = MultiSelect::new()
                    .with_prompt("Bindings")
                    .items_checked(languages.iter().copied())
                    .interact()?
                    .into_iter()
                    .map(|i| languages[i].0);

                let out = Bindings::with_enabled_languages(enabled)
                    .expect("unexpected unsupported language");
                anyhow::Ok(out)
            };

            let choices = [
                "name",
                "camelcase",
                "title",
                "description",
                "repository",
                "funding",
                "scope",
                "file_types",
                "version",
                "license",
                "author",
                "email",
                "url",
                "namespace",
                "bindings",
                "exit",
            ];

            macro_rules! set_choice {
                ($choice:expr) => {
                    match $choice {
                        "name" => opts.name = name()?,
                        "camelcase" => opts.camelcase = camelcase_name(&opts.name)?,
                        "title" => opts.title = title(&opts.name)?,
                        "description" => opts.description = description(&opts.name)?,
                        "repository" => opts.repository = Some(repository(&opts.name)?),
                        "funding" => opts.funding = funding()?,
                        "scope" => opts.scope = scope(&opts.name)?,
                        "file_types" => opts.file_types = file_types(&opts.name)?,
                        "version" => opts.version = initial_version()?,
                        "license" => opts.license = license()?,
                        "author" => opts.author = author()?,
                        "email" => opts.email = email()?,
                        "url" => opts.url = url()?,
                        "namespace" => opts.namespace = Some(namespace()?),
                        "bindings" => opts.bindings = bindings()?,
                        "exit" => break,
                        _ => unreachable!(),
                    }
                };
            }

            // Initial configuration
            for choice in choices.iter().take(choices.len() - 1) {
                set_choice!(*choice);
            }

            // Loop for editing the configuration
            loop {
                info!(
                    "Your current configuration:\n{}",
                    serde_json::to_string_pretty(&opts)?
                );

                if Confirm::with_theme(&ColorfulTheme::default())
                    .with_prompt("Does the config above look correct?")
                    .interact()?
                {
                    break;
                }

                let idx = FuzzySelect::with_theme(&ColorfulTheme::default())
                    .with_prompt("Which field would you like to change?")
                    .items(choices)
                    .interact()?;

                set_choice!(choices[idx]);
            }

            (opts.name.clone(), Some(opts))
        } else {
            let old_config = fs::read_to_string(current_dir.join("tree-sitter.json"))
                .with_context(|| "Failed to read tree-sitter.json")?;

            let mut json = serde_json::from_str::<TreeSitterJSON>(&old_config)?;
            if json.schema.is_none() {
                json.schema = Some(TREE_SITTER_JSON_SCHEMA.to_string());
            }

            let new_config = format!("{}\n", serde_json::to_string_pretty(&json)?);
            // Write the re-serialized config back, as newly added optional boolean fields
            // will be included with explicit `false`s rather than implicit `null`s
            if self.update && !old_config.trim().eq(new_config.trim()) {
                info!("Updating tree-sitter.json");
                fs::write(
                    current_dir.join("tree-sitter.json"),
                    serde_json::to_string_pretty(&json)?,
                )
                .with_context(|| "Failed to write tree-sitter.json")?;
            }

            (json.grammars.swap_remove(0).name, None)
        };

        generate_grammar_files(
            current_dir,
            &language_name,
            self.update,
            json_config_opts.as_ref(),
        )?;

        Ok(())
    }
}

impl Generate {
    fn run(self, mut loader: loader::Loader, current_dir: &Path) -> Result<()> {
        if self.log {
            logger::enable_debug();
        }
        let json_summary = if self.json {
            warn!("--json is deprecated, use --json-summary instead");
            true
        } else {
            self.json_summary
        };

        let mut diagnostics = Vec::new();
        let profile = self
            .profile
            .as_ref()
            .map(|path| -> Result<tree_sitter_generate::GenerationProfile> {
                let bytes = fs::read(path)
                    .with_context(|| format!("Failed to read profile {}", path.display()))?;
                serde_json::from_slice(&bytes)
                    .with_context(|| format!("Failed to parse profile {}", path.display()))
            })
            .transpose()?;
        let result = tree_sitter_generate::generate_parser_in_directory_with_profile(
            current_dir,
            self.output.as_deref(),
            self.grammar_path.as_deref(),
            self.report_states_for_rule.as_deref(),
            self.js_runtime.as_deref(),
            !self.no_parser,
            if self.disable_optimizations {
                OptLevel::empty()
            } else {
                OptLevel::default()
            },
            &mut diagnostics,
            profile.as_ref(),
        );
        if json_summary {
            #[derive(serde::Serialize)]
            struct Envelope<'a> {
                diagnostics: &'a [Diagnostic],
                error: Option<&'a GenerateError>,
            }
            let envelope = Envelope {
                diagnostics: &diagnostics,
                error: result.as_ref().err(),
            };
            eprintln!("{}", serde_json::to_string_pretty(&envelope)?);
            if result.is_err() {
                // Exit early to prevent errors from being printed a second time in the caller
                std::process::exit(1);
            }
        } else {
            for d in &diagnostics {
                warn!("{d}");
            }
            if let Err(err) = result {
                // Removes extra context associated with the error
                Err(anyhow!(err.to_string())).with_context(|| "Error when generating parser")?;
            }
        }

        if self.build {
            warn!("--build is deprecated, use the `build` command");
            if let Some(path) = self.libdir {
                loader = loader::Loader::with_parser_lib_path(path);
            }
            loader.debug_build(self.debug_build);
            loader.languages_at_path(current_dir)?;
        }
        Ok(())
    }
}

impl Profile {
    fn run(self, loader: &loader::Loader, current_dir: &Path) -> Result<()> {
        let src = current_dir.join("src");
        let parser_path = src.join("parser.c");
        let source = fs::read_to_string(&parser_path)
            .with_context(|| format!("Failed to read {}", parser_path.display()))?;
        tree_sitter_cli::generation_profile::ensure_unprofiled_source(&source)?;
        let library_directory = tempfile::tempdir()?;
        let library_path = library_directory
            .path()
            .join(format!("parser.{}", env::consts::DLL_EXTENSION));
        let language = loader.load_language_at_path(loader::CompileConfig::new(
            &src,
            None,
            Some(library_path),
        ))?;
        let metadata = language.metadata().context(
            "profile requires a parser generated by this CLI; run tree-sitter generate first",
        )?;
        let grammar_path = src.join("grammar.json");
        let (_, baseline) = tree_sitter_generate::generate_parser_for_grammar(
            &fs::read_to_string(&grammar_path)
                .with_context(|| format!("Failed to read {}", grammar_path.display()))?,
            Some((
                metadata.major_version,
                metadata.minor_version,
                metadata.patch_version,
            )),
            if self.disable_optimizations {
                OptLevel::empty()
            } else {
                OptLevel::default()
            },
            &mut Vec::new(),
        )?;
        anyhow::ensure!(
            source == baseline,
            "profile requires unprofiled source from this generator; run tree-sitter generate with matching optimization options first"
        );
        let (languages, known_languages) = if self.corpus.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            Self::corpus_languages(current_dir, language.name().unwrap_or_default())?
        };
        let mut inputs = Vec::new();
        let mut corpus_examples = 0;
        for path in self.corpus {
            corpus_examples += tree_sitter_cli::generation_profile::collect_corpus_inputs(
                test::parse_tests(&path)?,
                &languages,
                &known_languages,
                &mut inputs,
            );
        }
        let corpus_inputs = inputs.len();
        let source_files = self.paths.len();
        for path in self.paths {
            inputs.push(
                fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))?,
            );
        }
        anyhow::ensure!(
            !inputs.is_empty(),
            "profile requires at least one training input"
        );
        let mut parser = Parser::new();
        parser.set_language(&language)?;
        let profile =
            tree_sitter_cli::generation_profile::record_profile(&mut parser, &source, &inputs)?;
        let parse_actions = profile.parse_states.iter().sum::<u64>();
        fs::write(&self.output, serde_json::to_vec(&profile)?)
            .with_context(|| format!("Failed to write profile {}", self.output.display()))?;
        info!(
            "Recorded a generation profile from {corpus_inputs} corpus inputs ({corpus_examples} examples examined) and {source_files} source files; {parse_actions} parse actions recorded"
        );
        Ok(())
    }

    fn corpus_languages(grammar_path: &Path, fallback: &str) -> Result<(Vec<String>, Vec<String>)> {
        let selected_path = std::path::absolute(grammar_path)?;
        let grammar_path = selected_path.canonicalize()?;
        for root in selected_path.ancestors() {
            if root.join("tree-sitter.json").exists() {
                if let Ok(configuration) = loader::TreeSitterJSON::from_file(root) {
                    let mut known = configuration
                        .grammars
                        .iter()
                        .map(|grammar| grammar.name.clone())
                        .collect::<Vec<_>>();
                    let languages = configuration
                        .grammars
                        .into_iter()
                        .filter(|grammar| {
                            root.join(grammar.path.as_deref().unwrap_or_else(|| Path::new(".")))
                                .canonicalize()
                                .is_ok_and(|path| path == grammar_path)
                        })
                        .map(|grammar| grammar.name)
                        .collect::<Vec<_>>();
                    if !languages.is_empty() {
                        return Ok((languages, known));
                    }
                    known.push(fallback.to_owned());
                    return Ok((vec![fallback.to_owned()], known));
                }
                break;
            }
        }
        Ok((vec![fallback.to_owned()], vec![fallback.to_owned()]))
    }
}

impl Build {
    fn run(self, mut loader: loader::Loader, current_dir: &Path) -> Result<()> {
        let grammar_path = current_dir.join(self.path.unwrap_or_default());

        loader.debug_build(self.debug);
        if let Some(level) = self.optimization {
            loader.optimization_level(level);
        }
        loader.verbose_build(self.verbose);

        if self.wasm {
            let output_path = self.output.map(|path| current_dir.join(path));
            wasm::compile_language_to_wasm(&loader, &grammar_path, current_dir, output_path)?;
        } else {
            let output_path = if let Some(ref path) = self.output {
                let path = Path::new(path);
                let full_path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    current_dir.join(path)
                };
                let parent_path = full_path
                    .parent()
                    .context("Output path must have a parent")?;
                let name = full_path
                    .file_name()
                    .context("Output path must have a filename")?;
                fs::create_dir_all(parent_path).context("Failed to create output path")?;
                let mut canon_path = parent_path.canonicalize().context("Invalid output path")?;
                canon_path.push(name);
                canon_path
            } else {
                let file_name = grammar_path
                    .file_stem()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .strip_prefix("tree-sitter-")
                    .unwrap_or("parser");
                current_dir
                    .join(file_name)
                    .with_extension(env::consts::DLL_EXTENSION)
            };

            let flags: &[&str] = match (self.reuse_allocator, self.debug) {
                (true, true) => &["TREE_SITTER_REUSE_ALLOCATOR", "TREE_SITTER_DEBUG"],
                (true, false) => &["TREE_SITTER_REUSE_ALLOCATOR"],
                (false, true) => &["TREE_SITTER_DEBUG"],
                (false, false) => &[],
            };

            loader.force_rebuild(true);

            loader
                .compile_parser_at_path(&grammar_path, output_path, flags)
                .context("Failed to compile parser")?;
        }
        Ok(())
    }
}

impl Parse {
    fn run(self, mut loader: loader::Loader, current_dir: &Path) -> Result<()> {
        let config = Config::load(self.config_path.clone())?;
        let json_summary = if self.json {
            warn!("--json is deprecated, use --json-summary instead");
            true
        } else {
            self.json_summary
        };
        let parse_theme = if paint::color_enabled() {
            config
                .get::<parse::Config>()
                .with_context(|| "Failed to parse CST theme")?
                .parse_theme
                .unwrap_or_default()
                .into()
        } else {
            ParseTheme::empty()
        };
        let cancellation_flag = util::cancel_on_signal();
        let mut parser = self.init_parser(&mut loader)?;
        let mut stats = parse::ParseStats::default();
        let mut options =
            self.build_options(json_summary, &cancellation_flag, &mut stats, &parse_theme);

        let lib_info = get_lib_info(self.lib_path.as_ref(), self.lang_name.as_ref(), current_dir);
        if lib_info.is_none() {
            loader.find_all_languages(&config.get()?)?;
        }

        let input = get_input(
            self.paths_file.as_deref(),
            self.paths.clone(),
            self.test_number,
            &cancellation_flag,
        )?;
        self.run_input(
            input,
            (&mut loader, current_dir, lib_info.as_ref()),
            &mut parser,
            &mut options,
        )?;
        let has_error = summarize_parses(&mut stats, self.stat || json_summary);

        if self.stat {
            println!("\n{}", stats.cumulative_stats);
        }
        if json_summary {
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        if has_error {
            return Err(anyhow!(""));
        }
        Ok(())
    }

    fn build_options<'a>(
        &self,
        json_summary: bool,
        cancellation_flag: &'a std::sync::atomic::AtomicUsize,
        stats: &'a mut parse::ParseStats,
        parse_theme: &'a ParseTheme,
    ) -> ParseFileOptions<'a> {
        ParseFileOptions {
            output: if self.output_dot {
                ParseOutput::Dot
            } else if self.output_xml {
                ParseOutput::Xml
            } else if self.output_cst {
                ParseOutput::Cst
            } else if self.quiet || json_summary {
                ParseOutput::Quiet
            } else {
                ParseOutput::Normal
            },
            print_time: self.time,
            timeout: self.timeout.unwrap_or_default(),
            stats,
            debug: match self.debug {
                None => ParseDebugType::Quiet,
                Some(None) => ParseDebugType::Normal,
                Some(Some(specifier)) => specifier,
            },
            debug_graph: self.debug_graph,
            cancellation_flag: Some(cancellation_flag),
            encoding: self.encoding.as_ref().map(|e| match e {
                Encoding::Utf8 => ffi::TSInputEncodingUTF8,
                Encoding::Utf16LE => ffi::TSInputEncodingUTF16LE,
                Encoding::Utf16BE => ffi::TSInputEncodingUTF16BE,
            }),
            open_log: self.open_log,
            no_ranges: self.no_ranges,
            parse_theme,
        }
    }

    fn init_parser(&self, loader: &mut loader::Loader) -> Result<Parser> {
        loader.debug_build(self.debug_build);
        loader.force_rebuild(self.rebuild || self.grammar_path.is_some());
        if self.wasm {
            checked_wasm!({
                let mut parser = Parser::new();
                let engine = tree_sitter::wasmtime::Engine::default();
                parser.set_wasm_store(tree_sitter::WasmStore::new(&engine)?)?;
                loader.use_wasm(&engine);
                return Ok(parser);
            });
        }
        Ok(Parser::new())
    }

    fn run_input(
        &self,
        input: CliInput,
        (loader, current_dir, lib_info): (&mut loader::Loader, &Path, Option<&(PathBuf, &str)>),
        parser: &mut Parser,
        options: &mut ParseFileOptions<'_>,
    ) -> Result<()> {
        match input {
            CliInput::Paths(paths) => {
                let max_len = paths
                    .iter()
                    .map(|p| p.to_string_lossy().chars().count())
                    .max()
                    .unwrap_or(0);
                options.stats.source_count = paths.len();
                for path in &paths {
                    let path = Path::new(path);
                    let language = loader
                        .select_language(Some(path), current_dir, self.scope.as_deref(), lib_info)
                        .with_context(|| {
                            anyhow!("Failed to load language for path \"{}\"", path.display())
                        })?;
                    parse::parse_file_at_path(
                        parser,
                        &language,
                        path,
                        &path.display().to_string(),
                        max_len,
                        options,
                    )?;
                }
                Ok(())
            }
            CliInput::Test {
                name,
                contents,
                languages,
            } => {
                let language =
                    self.select_test_language(loader, current_dir, lib_info, &languages)?;
                parse_temp_source(&language, &name, &contents, parser, options)
            }
            CliInput::Stdin(contents) => {
                println!();
                let language =
                    loader.select_language(None, current_dir, self.scope.as_deref(), lib_info)?;
                parse_temp_source(&language, "stdin", &contents, parser, options)
            }
        }
    }

    fn select_test_language(
        &self,
        loader: &mut loader::Loader,
        current_dir: &Path,
        lib_info: Option<&(PathBuf, &str)>,
        language_names: &[Box<str>],
    ) -> Result<tree_sitter::Language> {
        if let Some(ref lib_path) = self.lib_path {
            loader
                .select_language(None, current_dir, self.scope.as_deref(), lib_info)
                .with_context(|| {
                    anyhow!(
                        "Failed to load language for path \"{}\"",
                        lib_path.display()
                    )
                })
        } else {
            let languages = loader.languages_at_path(current_dir)?;
            languages
                .iter()
                .find(|(_, n)| language_names.contains(&Box::from(n.as_str())))
                .or_else(|| languages.first())
                .map(|(l, _)| l.clone())
                .ok_or_else(|| anyhow!("No language found"))
        }
    }
}

fn parse_temp_source(
    language: &tree_sitter::Language,
    name: &str,
    contents: &[u8],
    parser: &mut Parser,
    options: &mut ParseFileOptions<'_>,
) -> Result<()> {
    let path = get_tmp_source_file(contents)?;
    parse::parse_file_at_path(parser, language, &path, name, name.chars().count(), options)?;
    path.close().map_err(Into::into)
}

fn summarize_parses(stats: &mut parse::ParseStats, accumulate: bool) -> bool {
    let mut has_error = false;
    for result in &stats.parse_summaries {
        has_error |= !result.successful;
        if accumulate {
            stats.cumulative_stats.total_parses += 1;
            stats.cumulative_stats.successful_parses += usize::from(result.successful);
            if let (Some(duration), Some(bytes)) = (result.duration, result.bytes) {
                stats.cumulative_stats.total_bytes += bytes;
                stats.cumulative_stats.total_duration += duration;
            }
        }
    }
    has_error
}

/// In case an error is encountered, prints out the contents of `test_summary` and
/// propagates the error
fn check_test(
    test_result: Result<()>,
    test_summary: &TestSummary,
    json_summary: bool,
) -> Result<()> {
    if let Err(e) = test_result {
        if json_summary {
            let json_summary = serde_json::to_string_pretty(test_summary)
                .expect("Failed to encode summary to JSON");
            println!("{json_summary}");
        } else {
            println!("{test_summary}");
        }

        Err(e)?;
    }

    Ok(())
}

impl Test {
    fn run(self, mut loader: loader::Loader, current_dir: &Path) -> Result<()> {
        let stat = self.stat.unwrap_or_default();

        loader.debug_build(self.debug_build);
        loader.force_rebuild(self.rebuild || self.grammar_path.is_some());

        let mut parser = Parser::new();

        if self.wasm {
            checked_wasm!({
                let engine = tree_sitter::wasmtime::Engine::default();
                parser
                    .set_wasm_store(tree_sitter::WasmStore::new(&engine).unwrap())
                    .unwrap();
                loader.use_wasm(&engine);
            });
        }

        let languages = load_test_languages(
            &mut loader,
            current_dir,
            self.lib_path.as_ref(),
            self.lang_name.as_ref(),
        )?;
        let language = &languages
            .first()
            .ok_or_else(|| anyhow!("No language found"))?
            .0;
        parser.set_language(language)?;

        let mut test_summary =
            TestSummary::new(stat, self.update, self.overview_only, self.json_summary);
        test_summary.use_markers = self.show_diff_markers;

        let test_corpus_dir = current_dir.join("test").join("corpus");
        if test_corpus_dir.is_dir() {
            let opts = TestOptions {
                path: test_corpus_dir,
                debug: self.debug,
                debug_graph: self.debug_graph,
                include: self.include,
                exclude: self.exclude,
                file_name: self.file_name,
                update: self.update,
                open_log: self.open_log,
                languages: languages.iter().map(|(l, n)| (n.as_str(), l)).collect(),
                show_fields: self.show_fields,
                overview_only: self.overview_only,
            };

            check_test(
                test::run_tests_at_path(&mut parser, &opts, &mut test_summary),
                &test_summary,
                self.json_summary,
            )?;
        } else {
            warn!("Test corpus not found at {}", test_corpus_dir.display());
        }

        print_test_summary(&test_summary, self.json_summary);
        Ok(())
    }
}

fn load_test_languages(
    loader: &mut loader::Loader,
    current_dir: &Path,
    lib_path: Option<&PathBuf>,
    lang_name: Option<&String>,
) -> Result<Vec<(tree_sitter::Language, String)>> {
    let lib_info = get_lib_info(lib_path, lang_name, current_dir);
    if let Some((lib_path, language_name)) = &lib_info {
        let language = loader
            .select_language(None, current_dir, None, lib_info.as_ref())
            .with_context(|| {
                anyhow!(
                    "Failed to load language for path \"{}\"",
                    lib_path.display()
                )
            })?;
        Ok(loader.languages_at_path_using(current_dir, language_name, &language)?)
    } else {
        Ok(loader.languages_at_path(current_dir)?)
    }
}

fn print_test_summary(test_summary: &TestSummary, json_summary: bool) {
    if json_summary {
        let json = serde_json::to_string_pretty(test_summary)
            .expect("Failed to encode test summary to JSON");
        println!("{json}");
    } else {
        println!("{test_summary}");
    }
}

impl Version {
    fn run(self, current_dir: PathBuf) -> Result<()> {
        Ok(version::Version::new(self.version, current_dir, self.bump).run()?)
    }
}

impl Fuzz {
    fn run(self, mut loader: loader::Loader, current_dir: &Path) -> Result<()> {
        loader.sanitize_build(true);
        loader.force_rebuild(self.rebuild || self.grammar_path.is_some());

        let (language, language_name) = if let Some(ref lib_path) = self.lib_path {
            let lib_info = get_lib_info(Some(lib_path), self.lang_name.as_ref(), current_dir)
                .with_context(|| anyhow!("No language name found for {}", lib_path.display()))?;
            let language = loader
                .select_language(None, current_dir, None, Some(&lib_info))
                .with_context(|| {
                    anyhow!(
                        "Failed to load language for path \"{}\"",
                        lib_path.display()
                    )
                })?;
            (language, lib_info.1.to_string())
        } else {
            loader
                .languages_at_path(current_dir)?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("No language found"))?
        };

        let mut fuzz_options = FuzzOptions {
            skipped: self.skip,
            subdir: self.subdir,
            edits: self.edits.unwrap_or(*EDIT_COUNT),
            iterations: self.iterations.unwrap_or(*ITERATION_COUNT),
            include: self.include,
            exclude: self.exclude,
            log_graphs: self.log_graphs || *LOG_GRAPH_ENABLED,
            log: self.log || *LOG_ENABLED,
        };

        fuzz_language_corpus(
            &language,
            &language_name,
            *START_SEED,
            current_dir,
            &mut fuzz_options,
        );
        Ok(())
    }
}

impl Playground {
    fn run(self, current_dir: &Path) -> Result<()> {
        let grammar_path = self.grammar_path.as_deref().map_or(current_dir, Path::new);

        if let Some(export_path) = self.export {
            playground::export(grammar_path, &export_path)?;
        } else {
            let open_in_browser = !self.quiet;
            playground::serve(grammar_path, open_in_browser)?;
        }

        Ok(())
    }
}

impl DumpLanguages {
    fn run(self, mut loader: loader::Loader) -> Result<()> {
        let config = Config::load(self.config_path)?;
        let loader_config = config.get()?;
        loader.find_all_languages(&loader_config)?;
        for (configuration, language_path) in loader.get_all_language_configurations() {
            info!(
                concat!(
                    "name: {}\n",
                    "scope: {}\n",
                    "parser: {}\n",
                    "file_types: {:?}\n",
                    "content_regex: {:?}\n",
                ),
                configuration.language_name,
                configuration.scope.as_ref().unwrap_or(&String::new()),
                language_path.display(),
                configuration.file_types,
                configuration.content_regex,
            );
        }
        Ok(())
    }
}

impl Complete {
    fn run(self, cli: &mut Command) {
        let name = cli.get_name().to_string();
        let mut stdout = std::io::stdout();

        match self.shell {
            Shell::Bash => generate(clap_complete::shells::Bash, cli, &name, &mut stdout),
            Shell::Elvish => generate(clap_complete::shells::Elvish, cli, &name, &mut stdout),
            Shell::Fish => generate(clap_complete::shells::Fish, cli, &name, &mut stdout),
            Shell::PowerShell => {
                generate(clap_complete::shells::PowerShell, cli, &name, &mut stdout);
            }
            Shell::Zsh => generate(clap_complete::shells::Zsh, cli, &name, &mut stdout),
            Shell::Nushell => generate(clap_complete_nushell::Nushell, cli, &name, &mut stdout),
        }
    }
}

fn main() {
    let result = run();
    if let Err(err) = &result {
        // Ignore BrokenPipe errors
        if let Some(error) = err.downcast_ref::<std::io::Error>()
            && error.kind() == std::io::ErrorKind::BrokenPipe
        {
            return;
        }
        if !err.to_string().is_empty() {
            error!("{err:?}");
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    logger::init();

    let mut cli = build_cli();
    let command = Commands::from_arg_matches(&cli.clone().get_matches())?;
    let current_dir = command_grammar_dir(&command);
    let loader = loader::Loader::new()?;
    dispatch_command(command, loader, current_dir, &mut cli)
}

fn dispatch_command(
    command: Commands,
    loader: loader::Loader,
    current_dir: PathBuf,
    cli: &mut Command,
) -> Result<()> {
    match command {
        Commands::InitConfig(_) => InitConfig::run()?,
        Commands::Init(init_options) => init_options.run(&current_dir)?,
        Commands::Generate(generate_options) => generate_options.run(loader, &current_dir)?,
        Commands::Profile(profile_options) => profile_options.run(&loader, &current_dir)?,
        Commands::Build(build_options) => build_options.run(loader, &current_dir)?,
        Commands::Parse(parse_options) => parse_options.run(loader, &current_dir)?,
        Commands::Test(test_options) => test_options.run(loader, &current_dir)?,
        Commands::Version(version_options) => version_options.run(current_dir)?,
        Commands::Fuzz(fuzz_options) => fuzz_options.run(loader, &current_dir)?,
        Commands::Playground(playground_options) => playground_options.run(&current_dir)?,
        Commands::DumpLanguages(dump_options) => dump_options.run(loader)?,
        Commands::Complete(complete_options) => complete_options.run(cli),
    }

    Ok(())
}

fn build_cli() -> Command {
    let version = BUILD_SHA.map_or_else(
        || BUILD_VERSION.to_string(),
        |build_sha| format!("{BUILD_VERSION} ({build_sha})"),
    );
    let version = if let Some(fork_version) = option_env!("TREE_SITTER_FORK_VERSION") {
        format!("{version} (WillBooster {fork_version})")
    } else {
        version
    };

    let cli = Command::new("tree-sitter")
        .help_template(concat!(
            "\n",
            "{before-help}{name} {version}\n",
            "{author-with-newline}{about-with-newline}\n",
            "{usage-heading} {usage}\n",
            "\n",
            "{all-args}{after-help}\n",
            "\n"
        ))
        .version(version)
        .subcommand_required(true)
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .disable_colored_help(false);
    Commands::augment_subcommands(cli)
}

fn command_grammar_dir(command: &Commands) -> PathBuf {
    match command {
        Commands::Init(Init { grammar_path, .. })
        | Commands::Profile(Profile { grammar_path, .. })
        | Commands::Parse(Parse { grammar_path, .. })
        | Commands::Test(Test { grammar_path, .. })
        | Commands::Version(Version { grammar_path, .. })
        | Commands::Fuzz(Fuzz { grammar_path, .. })
        | Commands::Playground(Playground { grammar_path, .. }) => grammar_path,
        Commands::Build(_)
        | Commands::Generate(_)
        | Commands::InitConfig(_)
        | Commands::DumpLanguages(_)
        | Commands::Complete(_) => &None,
    }
    .as_ref()
    .map_or_else(|| env::current_dir().unwrap(), Clone::clone)
}

#[must_use]
const fn get_styles() -> clap::builder::Styles {
    clap::builder::Styles::styled()
        .usage(
            Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Yellow))),
        )
        .header(
            Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Yellow))),
        )
        .literal(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green))))
        .invalid(
            Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Red))),
        )
        .error(
            Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Red))),
        )
        .valid(
            Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Green))),
        )
        .placeholder(Style::new().fg_color(Some(Color::Ansi(AnsiColor::White))))
}

/// Utility to extract the shared library path and language function name from user-provided
/// arguments if present.
fn get_lib_info<'a>(
    lib_path: Option<&'a PathBuf>,
    language_name: Option<&'a String>,
    current_dir: &Path,
) -> Option<(PathBuf, &'a str)> {
    if let Some(lib_path) = lib_path {
        let absolute_lib_path = if lib_path.is_absolute() {
            lib_path.clone()
        } else {
            current_dir.join(lib_path)
        };
        // Use the user-specified name if present, otherwise try to derive it from
        // the lib path
        match (
            language_name.map(std::string::String::as_str),
            lib_path.file_stem().and_then(|s| s.to_str()),
        ) {
            (Some(name), _) | (None, Some(name)) => Some((absolute_lib_path, name)),
            _ => None,
        }
    } else {
        None
    }
}
