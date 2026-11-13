use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use log::{error, info};
use rand::RngExt;
use regex::Regex;
use tree_sitter::Language;

pub mod allocations;
pub mod corpus_test;
pub mod edits;
pub mod random;

use crate::{
    fuzz::{
        corpus_test::{check_consistent_sizes, get_parser, set_included_ranges},
        edits::{apply_edit, get_random_edit, invert_edit},
        random::Rand,
    },
    test::{DiffKey, TestDiff, TestEntry, TestExpectation, parse_tests, render_test_output},
};

pub static LOG_ENABLED: LazyLock<bool> = LazyLock::new(|| env::var("TREE_SITTER_LOG").is_ok());

pub static LOG_GRAPH_ENABLED: LazyLock<bool> =
    LazyLock::new(|| env::var("TREE_SITTER_LOG_GRAPHS").is_ok());

pub static LANGUAGE_FILTER: LazyLock<Option<String>> =
    LazyLock::new(|| env::var("TREE_SITTER_LANGUAGE").ok());

pub static EXAMPLE_INCLUDE: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex_env_var("TREE_SITTER_EXAMPLE_INCLUDE"));

pub static EXAMPLE_EXCLUDE: LazyLock<Option<Regex>> =
    LazyLock::new(|| regex_env_var("TREE_SITTER_EXAMPLE_EXCLUDE"));

pub static START_SEED: LazyLock<usize> = LazyLock::new(new_seed);

pub const DEFAULT_EDIT_COUNT: usize = 3;
pub static EDIT_COUNT: LazyLock<usize> =
    LazyLock::new(|| int_env_var("TREE_SITTER_EDITS").unwrap_or(DEFAULT_EDIT_COUNT));

pub const DEFAULT_ITERATION_COUNT: usize = 10;
pub static ITERATION_COUNT: LazyLock<usize> =
    LazyLock::new(|| int_env_var("TREE_SITTER_ITERATIONS").unwrap_or(DEFAULT_ITERATION_COUNT));

fn int_env_var(name: &'static str) -> Option<usize> {
    env::var(name).ok().and_then(|e| e.parse().ok())
}

fn regex_env_var(name: &'static str) -> Option<Regex> {
    env::var(name).ok().and_then(|e| Regex::new(&e).ok())
}

#[must_use]
pub fn new_seed() -> usize {
    int_env_var("TREE_SITTER_SEED").unwrap_or_else(|| {
        let mut rng = rand::rng();
        let seed = rng.random_range(0..=usize::MAX);
        eprintln!("fuzz seed: {seed}");
        seed
    })
}

pub struct FuzzOptions {
    pub skipped: Option<Vec<String>>,
    pub subdir: Option<PathBuf>,
    pub edits: usize,
    pub iterations: usize,
    pub include: Option<Regex>,
    pub exclude: Option<Regex>,
    pub log_graphs: bool,
    pub log: bool,
}

pub fn fuzz_language_corpus(
    language: &Language,
    language_name: &str,
    start_seed: usize,
    grammar_dir: &Path,
    options: &mut FuzzOptions,
) {
    let Some(tests) = load_corpus_tests(language_name, grammar_dir, options) else {
        return;
    };
    let get_test_name = |test: &FlattenedTest| format!("{language_name} - {}", test.name);
    let mut skipped = options
        .skipped
        .take()
        .unwrap_or_default()
        .into_iter()
        .chain(tests.iter().filter(|t| t.skip()).map(get_test_name))
        .map(|x| (x, 0))
        .collect::<HashMap<String, usize>>();

    let log_seed = env::var("TREE_SITTER_LOG_SEED").is_ok();
    let dump_edits = env::var("TREE_SITTER_DUMP_EDITS").is_ok();
    if log_seed {
        info!("  start seed: {start_seed}");
    }

    println!();
    let ctx = FuzzRunContext {
        language,
        start_seed,
        options,
        log_seed,
        dump_edits,
    };
    let mut failure_count = 0;
    for (test_index, test) in tests.iter().enumerate() {
        let test_name = get_test_name(test);
        if let Some(counter) = skipped.get_mut(test_name.as_str()) {
            println!("  {test_index}. {test_name} - SKIPPED");
            *counter += 1;
            continue;
        }
        println!("  {test_index}. {test_name}");
        if !run_fuzz_test(&ctx, test, &test_name, test_index) {
            failure_count += 1;
        }
    }

    if failure_count != 0 {
        info!("{failure_count} {language_name} corpus tests failed fuzzing");
    }
    report_unmatched_skips(&mut skipped);
}

struct FuzzRunContext<'a> {
    language: &'a Language,
    start_seed: usize,
    options: &'a FuzzOptions,
    log_seed: bool,
    dump_edits: bool,
}

fn load_corpus_tests(
    language_name: &str,
    grammar_dir: &Path,
    options: &mut FuzzOptions,
) -> Option<Vec<FlattenedTest>> {
    let subdir = options.subdir.take().unwrap_or_default();
    let corpus_dir = grammar_dir.join(subdir).join("test").join("corpus");
    if !corpus_dir.exists() || !corpus_dir.is_dir() {
        error!(
            "No corpus directory found, ensure that you have a `test/corpus` directory in your grammar directory with at least one test file."
        );
        return None;
    }
    if std::fs::read_dir(&corpus_dir).unwrap().count() == 0 {
        error!(
            "No corpus files found in `test/corpus`, ensure that you have at least one test file in your corpus directory."
        );
        return None;
    }

    let mut main_tests = parse_tests(&corpus_dir).unwrap();
    match main_tests {
        TestEntry::Group {
            ref mut children, ..
        } => {
            children.retain_mut(|child| retain_language_tests(child, language_name));
        }
        TestEntry::Example { .. } => unreachable!(),
    }
    Some(flatten_tests(
        main_tests,
        options.include.as_ref(),
        options.exclude.as_ref(),
    ))
}

fn retain_language_tests(entry: &mut TestEntry, language_name: &str) -> bool {
    match entry {
        TestEntry::Example { attributes, .. } => {
            attributes.languages[0].is_empty()
                || attributes
                    .languages
                    .iter()
                    .any(|lang| lang.as_ref() == language_name)
        }
        TestEntry::Group { children, .. } => {
            children.retain_mut(|child| retain_language_tests(child, language_name));
            !children.is_empty()
        }
    }
}

fn run_fuzz_test(
    ctx: &FuzzRunContext<'_>,
    test: &FlattenedTest,
    test_name: &str,
    test_index: usize,
) -> bool {
    let initial_passed = allocations::record_checked(|| {
        test.check_initial_parse(ctx.language, test_name, !test.error())
    })
    .unwrap_or_else(|e| {
        error!("{e}");
        false
    });
    if !initial_passed {
        return false;
    }

    for trial in 0..ctx.options.iterations {
        let passed =
            allocations::record_checked(|| run_fuzz_trial(ctx, test, test_name, test_index, trial))
                .unwrap_or_else(|e| {
                    error!("{e}");
                    false
                });
        if !passed {
            return false;
        }
    }
    true
}

fn run_fuzz_trial(
    ctx: &FuzzRunContext<'_>,
    test: &FlattenedTest,
    test_name: &str,
    test_index: usize,
    trial: usize,
) -> bool {
    let seed = ctx.start_seed + trial;
    let mut rand = Rand::new(seed);
    let mut log_session = None;
    let mut parser = get_parser(&mut log_session, "log.html");
    parser.set_language(ctx.language).unwrap();
    let mut input = test.input.clone();

    log_fuzz_input(ctx.options.log_graphs, &input);
    let undo_stack = apply_random_edits(&mut rand, &mut input, ctx.options.edits);
    record_trial_input(ctx, test_name, test_index, trial, seed, &input);

    set_included_ranges(&mut parser, &input, test.template_delimiters);
    let edited_tree = parser.parse(&input).unwrap();
    check_consistent_sizes(&edited_tree, &input);

    for edit in undo_stack.into_iter().rev() {
        apply_edit(&mut input, &edit);
    }
    log_fuzz_input(ctx.options.log_graphs, &input);

    set_included_ranges(&mut parser, &test.input, test.template_delimiters);
    let restored_tree = parser.parse(&input).unwrap();
    let actual_output =
        render_test_output(&input, &restored_tree, test.cst, test.has_fields).unwrap();

    if actual_output != test.output && !test.error() {
        println!("Incorrect parse for {test_name} - seed {seed}");
        DiffKey::print();
        println!("{}", TestDiff::new(&actual_output, &test.output));
        println!();
        return false;
    }

    check_consistent_sizes(&restored_tree, &input);
    true
}

fn record_trial_input(
    ctx: &FuzzRunContext<'_>,
    test_name: &str,
    test_index: usize,
    trial: usize,
    seed: usize,
    input: &[u8],
) {
    if ctx.log_seed {
        info!("   {test_index}.{trial:<2} seed: {seed}");
    }
    if ctx.dump_edits {
        fs::create_dir_all("fuzz").unwrap();
        fs::write(
            Path::new("fuzz").join(format!("edit.{seed}.{test_index}.{trial} {test_name}")),
            input,
        )
        .unwrap();
    }
    log_fuzz_input(ctx.options.log_graphs, input);
}

fn apply_random_edits(rand: &mut Rand, input: &mut Vec<u8>, max_edits: usize) -> Vec<edits::Edit> {
    let edit_count = rand.unsigned(max_edits);
    let mut undo_stack = Vec::with_capacity(edit_count + 1);
    for _ in 0..=edit_count {
        let edit = get_random_edit(rand, input);
        undo_stack.push(invert_edit(input, &edit));
        apply_edit(input, &edit);
    }
    undo_stack
}

fn log_fuzz_input(enabled: bool, input: &[u8]) {
    if enabled {
        info!("{}\n", String::from_utf8_lossy(input));
    }
}

fn report_unmatched_skips(skipped: &mut HashMap<String, usize>) {
    skipped.retain(|_, v| *v == 0);
    if !skipped.is_empty() {
        info!("Non matchable skip definitions:");
        for k in skipped.keys() {
            info!("  {k}");
        }
        panic!("Non matchable skip definitions need to be removed");
    }
}

pub struct FlattenedTest {
    pub name: String,
    pub input: Vec<u8>,
    pub output: String,
    pub languages: Vec<Box<str>>,
    pub expectation: TestExpectation,
    pub has_fields: bool,
    pub cst: bool,
    pub template_delimiters: Option<(&'static str, &'static str)>,
}

impl FlattenedTest {
    #[must_use]
    fn skip(&self) -> bool {
        self.expectation == TestExpectation::Skip
    }

    #[must_use]
    fn error(&self) -> bool {
        self.expectation == TestExpectation::Error
    }

    #[must_use]
    pub(crate) fn check_initial_parse(
        &self,
        language: &Language,
        display_name: &str,
        check_output: bool,
    ) -> bool {
        let mut log_session = None;
        let mut parser = get_parser(&mut log_session, "log.html");
        parser.set_language(language).unwrap();
        set_included_ranges(&mut parser, &self.input, self.template_delimiters);

        let tree = parser.parse(&self.input).unwrap();
        check_consistent_sizes(&tree, &self.input);

        if !check_output {
            return true;
        }

        let actual_output =
            render_test_output(&self.input, &tree, self.cst, self.has_fields).unwrap();

        if actual_output == self.output {
            true
        } else {
            println!("Incorrect initial parse for {display_name}");
            DiffKey::print();
            println!("{}", TestDiff::new(&actual_output, &self.output));
            println!();
            false
        }
    }
}

#[must_use]
pub fn flatten_tests(
    test: TestEntry,
    include: Option<&Regex>,
    exclude: Option<&Regex>,
) -> Vec<FlattenedTest> {
    fn helper(
        test: TestEntry,
        include: Option<&Regex>,
        exclude: Option<&Regex>,
        is_root: bool,
        prefix: &str,
        result: &mut Vec<FlattenedTest>,
    ) {
        match test {
            TestEntry::Example {
                mut name,
                input,
                output,
                has_fields,
                attributes,
                ..
            } => {
                if !prefix.is_empty() {
                    name.insert_str(0, " - ");
                    name.insert_str(0, prefix);
                }

                if let Some(include) = include {
                    if !include.is_match(&name) {
                        return;
                    }
                } else if let Some(exclude) = exclude
                    && exclude.is_match(&name)
                {
                    return;
                }

                result.push(FlattenedTest {
                    name,
                    input,
                    output,
                    languages: attributes.languages,
                    expectation: attributes.expectation,
                    has_fields,
                    cst: attributes.cst,
                    template_delimiters: None,
                });
            }
            TestEntry::Group {
                mut name, children, ..
            } => {
                if !is_root && !prefix.is_empty() {
                    name.insert_str(0, " - ");
                    name.insert_str(0, prefix);
                }
                for child in children {
                    helper(child, include, exclude, false, &name, result);
                }
            }
        }
    }
    let mut result = Vec::new();
    helper(test, include, exclude, true, "", &mut result);
    result
}
