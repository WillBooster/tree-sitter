use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Instant,
};

use anyhow::Context;
use log::info;
use tree_sitter::{Language, Parser};
#[cfg(feature = "wasm")]
use tree_sitter::{WasmStore, wasmtime};
use tree_sitter_loader::{CompileConfig, Loader};

include!("../src/tests/helpers/dirs.rs");

static LANGUAGE_FILTER: LazyLock<Option<String>> =
    LazyLock::new(|| env::var("TREE_SITTER_BENCHMARK_LANGUAGE_FILTER").ok());
static EXAMPLE_FILTER: LazyLock<Option<String>> =
    LazyLock::new(|| env::var("TREE_SITTER_BENCHMARK_EXAMPLE_FILTER").ok());
static REPETITION_COUNT: LazyLock<usize> = LazyLock::new(|| {
    env::var("TREE_SITTER_BENCHMARK_REPETITION_COUNT").map_or(5, |s| s.parse::<usize>().unwrap())
});
static WASM: LazyLock<bool> = LazyLock::new(|| env::var_os("TREE_SITTER_BENCHMARK_WASM").is_some());
static TEST_LOADER: LazyLock<Loader> =
    LazyLock::new(|| Loader::with_parser_lib_path(SCRATCH_DIR.clone()));
#[cfg(feature = "wasm")]
static WASM_ENGINE: LazyLock<wasmtime::Engine> = LazyLock::new(Default::default);

static EXAMPLE_PATHS_BY_LANGUAGE_DIR: LazyLock<BTreeMap<PathBuf, Vec<PathBuf>>> =
    LazyLock::new(|| {
        let mut result = BTreeMap::new();
        collect_example_paths(&mut result, &GRAMMARS_DIR);
        result
    });

fn main() {
    tree_sitter_cli::logger::init();

    let max_path_length = EXAMPLE_PATHS_BY_LANGUAGE_DIR
        .values()
        .flat_map(|e| {
            e.iter()
                .map(|s| s.file_name().unwrap().to_str().unwrap().len())
        })
        .max()
        .unwrap_or(0);

    info!("Benchmarking with {} repetitions", *REPETITION_COUNT);

    let mut parser = Parser::new();
    let mut all_normal_speeds = Vec::new();
    let mut all_error_speeds = Vec::new();

    for (language_path, example_paths) in EXAMPLE_PATHS_BY_LANGUAGE_DIR.iter() {
        let language_name = language_path.file_name().unwrap().to_str().unwrap();

        if LANGUAGE_FILTER
            .as_deref()
            .is_some_and(|filter| language_name != filter)
        {
            continue;
        }

        let (normal_speeds, error_speeds) = benchmark_language(
            language_path,
            language_name,
            example_paths,
            max_path_length,
            &mut parser,
        );
        all_normal_speeds.extend(normal_speeds);
        all_error_speeds.extend(error_speeds);
    }

    info!("\n  Overall");
    log_speed_summary("normal", &all_normal_speeds);
    log_speed_summary("errors", &all_error_speeds);
    info!("");
}

fn benchmark_language(
    language_path: &Path,
    language_name: &str,
    example_paths: &[PathBuf],
    max_path_length: usize,
    parser: &mut Parser,
) -> (Vec<usize>, Vec<usize>) {
    info!("\nLanguage: {language_name}");
    let language = if *WASM {
        get_wasm_language(language_name, parser)
    } else {
        get_language(language_path)
    };
    parser.set_language(&language).unwrap();

    info!("  Parsing Valid Code:");
    let normal_speeds = example_paths
        .iter()
        .filter(|path| matches_example_filter(path))
        .map(|path| {
            parse(path, max_path_length, |code| {
                parser.parse(code).expect("Failed to parse");
            })
        })
        .collect::<Vec<_>>();

    info!("  Parsing Invalid Code (mismatched languages):");
    let error_speeds = EXAMPLE_PATHS_BY_LANGUAGE_DIR
        .iter()
        .filter(|(other_path, _)| other_path.as_path() != language_path)
        .flat_map(|(_, paths)| paths.iter())
        .filter(|path| matches_example_filter(path))
        .map(|path| {
            parse(path, max_path_length, |code| {
                parser.parse(code).expect("Failed to parse");
            })
        })
        .collect::<Vec<_>>();

    log_speed_summary("normal", &normal_speeds);
    log_speed_summary("errors", &error_speeds);
    (normal_speeds, error_speeds)
}

fn collect_example_paths(result: &mut BTreeMap<PathBuf, Vec<PathBuf>>, dir: &Path) {
    if dir.join("grammar.js").exists() {
        let relative_path = dir.strip_prefix(GRAMMARS_DIR.as_path()).unwrap();
        let example_paths = result.entry(relative_path.to_owned()).or_default();
        if let Ok(example_files) = fs::read_dir(dir.join("examples")) {
            example_paths.extend(
                example_files
                    .map(|p| p.unwrap().path())
                    .filter(|p| p.is_file()),
            );
        }
        return;
    }

    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap().path();
        if entry.is_dir() {
            collect_example_paths(result, &entry);
        }
    }
}

fn matches_example_filter(path: &Path) -> bool {
    EXAMPLE_FILTER
        .as_deref()
        .is_none_or(|filter| path.to_str().unwrap().contains(filter))
}

fn log_speed_summary(kind: &str, speeds: &[usize]) {
    if let Some((average, worst)) = aggregate(speeds) {
        info!("  Average Speed ({kind}): {average} bytes/ms");
        info!("  Worst Speed ({kind}):   {worst} bytes/ms");
    }
}

fn aggregate(speeds: &[usize]) -> Option<(usize, usize)> {
    if speeds.is_empty() {
        return None;
    }
    let mut total = 0;
    let mut max = usize::MAX;
    for speed in speeds.iter().copied() {
        total += speed;
        if speed < max {
            max = speed;
        }
    }
    Some((total / speeds.len(), max))
}

fn parse(path: &Path, max_path_length: usize, mut action: impl FnMut(&[u8])) -> usize {
    let source_code = fs::read(path)
        .with_context(|| format!("Failed to read {}", path.display()))
        .unwrap();
    let time = Instant::now();
    for _ in 0..*REPETITION_COUNT {
        action(&source_code);
    }
    let duration = time.elapsed() / (*REPETITION_COUNT as u32);
    let duration_ns = duration.as_nanos();
    let speed = ((source_code.len() as u128) * 1_000_000) / duration_ns;
    info!(
        "    {:max_path_length$}\ttime {:>7.2} ms\t\tspeed {speed:>6} bytes/ms",
        path.file_name().unwrap().to_str().unwrap(),
        (duration_ns as f64) / 1e6,
    );
    speed as usize
}

fn get_language(path: &Path) -> Language {
    let src_path = GRAMMARS_DIR.join(path).join("src");
    let library_path = SCRATCH_DIR.join(format!(
        "benchmark-{}.{}",
        path.file_name().unwrap().to_str().unwrap(),
        env::consts::DLL_EXTENSION,
    ));
    let language = TEST_LOADER
        .load_language_at_path(CompileConfig::new(
            &src_path,
            None,
            Some(library_path.clone()),
        ))
        .with_context(|| format!("Failed to load language at path {}", src_path.display()))
        .unwrap();
    info!(
        "  Parser size: {} bytes of C, {} bytes compiled, {} states",
        fs::metadata(src_path.join("parser.c")).unwrap().len(),
        fs::metadata(library_path).unwrap().len(),
        language.parse_state_count(),
    );
    language
}

#[cfg(feature = "wasm")]
fn get_wasm_language(language_name: &str, parser: &mut Parser) -> Language {
    let wasm_language_name = language_name.replace('-', "_");
    let wasm_path = ROOT_DIR
        .join("target")
        .join("release")
        .join(format!("tree-sitter-{language_name}.wasm"));
    let wasm = fs::read(&wasm_path)
        .with_context(|| {
            format!(
                "Failed to read {}. Generate Wasm fixtures with `cargo xtask generate-fixtures --wasm`",
                wasm_path.display()
            )
        })
        .unwrap();
    let mut store = WasmStore::new(&WASM_ENGINE).expect("Failed to create Wasm store");
    let language = store
        .load_language(&wasm_language_name, &wasm)
        .with_context(|| format!("Failed to load Wasm language at {}", wasm_path.display()))
        .unwrap();
    info!(
        "  Parser size: {} bytes of Wasm, {} states",
        wasm.len(),
        language.parse_state_count(),
    );
    parser.set_wasm_store(store).unwrap();
    language
}

#[cfg(not(feature = "wasm"))]
fn get_wasm_language(_language_name: &str, _parser: &mut Parser) -> Language {
    panic!("Wasm benchmarking requires the `wasm` feature");
}
