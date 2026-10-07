use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, ensure};
use tree_sitter::Parser;
use tree_sitter_generate::{GenerationProfile, parser_fingerprint};

use crate::test::{TestEntry, TestExpectation};

pub fn collect_corpus_inputs(entry: TestEntry, language: &str, inputs: &mut Vec<Vec<u8>>) {
    match entry {
        TestEntry::Group { children, .. } => {
            for child in children {
                collect_corpus_inputs(child, language, inputs);
            }
        }
        TestEntry::Example {
            input, attributes, ..
        } => {
            if attributes.platform
                && attributes.expectation != TestExpectation::Skip
                && attributes
                    .languages
                    .iter()
                    .any(|name| name.is_empty() || name.as_ref() == language)
            {
                inputs.push(input);
            }
        }
    }
}

pub fn record_profile(
    parser: &mut Parser,
    source: &str,
    inputs: &[Vec<u8>],
) -> Result<GenerationProfile> {
    ensure!(
        !source
            .lines()
            .any(|line| line == GenerationProfile::SOURCE_MARKER),
        "profile requires unprofiled source; run tree-sitter generate --abi 16 first"
    );
    ensure!(
        parser
            .language()
            .is_some_and(|language| language.abi_version() >= 16),
        "profile requires an ABI 16 parser"
    );
    let count = |name: &str| -> Result<usize> {
        source
            .lines()
            .find_map(|line| line.strip_prefix(&format!("#define {name} ")))
            .context("profile requires an ABI 16 parser")?
            .parse()
            .context("invalid state count")
    };
    let state_count = count("STATE_COUNT")?;
    let counters = Arc::new(Mutex::new((
        vec![0u64; state_count],
        vec![0u64; count("LEX_STATE_COUNT")?],
        BTreeMap::<(u32, u32), u64>::new(),
        None::<u32>,
    )));
    let captured = counters.clone();
    parser.set_logger(Some(Box::new(move |_, message| {
        let Some(state) = message
            .split_once("from_state:")
            .or_else(|| message.split_once("state:"))
            .and_then(|(_, s)| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse::<usize>().ok())
        else {
            return;
        };
        let mut counters = captured.lock().unwrap();
        if ["shift ", "shift_extra ", "reduce ", "accept "]
            .iter()
            .any(|prefix| message.starts_with(prefix))
            && state < counters.0.len()
        {
            counters.0[state] += 1;
            if let Some(previous) = counters.3 {
                *counters.2.entry((previous, state as u32)).or_default() += 1;
            }
            counters.3 = Some(state as u32);
        } else if message.starts_with("lex_internal ") && state < counters.1.len() {
            counters.1[state] += 1;
        }
    })));
    let result = inputs.iter().try_for_each(|input| {
        counters.lock().unwrap().3 = None;
        parser
            .parse(input, None)
            .map(|_| ())
            .context("profile parse cancelled")
    });
    parser.set_logger(None);
    result?;
    let counters = counters.lock().unwrap();
    let profile = GenerationProfile {
        fingerprint: parser_fingerprint(source),
        parse_states: counters.0.clone(),
        lex_states: counters.1.clone(),
        edges: counters
            .2
            .iter()
            .map(|(&(a, b), &count)| (a, b, count))
            .collect(),
        max_dense_states: state_count.min(256),
    };
    drop(counters);
    Ok(profile)
}
