use std::{
    env, fs,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use tree_sitter::{InputEdit, Parser, Point, Tree};
use tree_sitter_loader::{CompileConfig, Loader};

fn main() -> Result<()> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let profile_path = if args.first().is_some_and(|arg| arg == "--profile") {
        ensure!(args.len() >= 2, "--profile requires an output path");
        let path = args[1].clone();
        args.drain(..2);
        Some(path)
    } else {
        None
    };
    ensure!(
        args.len() >= 4,
        "usage: parser_study [--profile <output.json>] <native|wasm> <language> <src-directory|wasm-file> <input>..."
    );
    let language_name = args[1].replace('-', "_");
    let mut parser = Parser::new();
    let language = match args[0].as_str() {
        "native" => {
            let src = Path::new(&args[2]);
            let loader = Loader::new()?;
            let output = src
                .parent()
                .context("source directory must have a parent")?
                .join(format!("parser.{}", env::consts::DLL_EXTENSION));
            let mut config = CompileConfig::new(src, None, Some(output));
            config.scanner_path = loader.get_scanner_path(src);
            config.name.clone_from(&language_name);
            loader.load_language_at_path_with_name(config)?
        }
        #[cfg(feature = "wasm")]
        "wasm" => {
            let engine = tree_sitter::wasmtime::Engine::default();
            let mut store = tree_sitter::WasmStore::new(&engine)?;
            let language = store.load_language(&language_name, &fs::read(&args[2])?)?;
            parser.set_wasm_store(store)?;
            language
        }
        #[cfg(not(feature = "wasm"))]
        "wasm" => {
            anyhow::bail!("Wasm measurements require building parser_study with --features wasm")
        }
        backend => anyhow::bail!("unsupported backend: {backend}"),
    };
    parser.set_language(&language)?;
    for path in &args[3..] {
        measure(&mut parser, path)?;
    }
    if let Some(path) = profile_path {
        record_profile(&mut parser, &args, &path)?;
    }
    Ok(())
}

fn record_profile(parser: &mut Parser, args: &[String], output: &str) -> Result<()> {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    let parser_path = if args[0] == "native" {
        Path::new(&args[2]).join("parser.c")
    } else {
        Path::new(&args[2])
            .parent()
            .context("Wasm file must have a parent")?
            .join("src/parser.c")
    };
    let source = fs::read_to_string(parser_path)?;
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
            .split_once("state:")
            .and_then(|(_, s)| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse::<usize>().ok())
        else {
            return;
        };
        let mut counters = captured.lock().unwrap();
        if message.starts_with("process ") && state < counters.0.len() {
            counters.0[state] += 1;
            if let Some(previous) = counters.3 {
                *counters.2.entry((previous, state as u32)).or_default() += 1;
            }
            counters.3 = Some(state as u32);
        } else if message.starts_with("lex_internal ") && state < counters.1.len() {
            counters.1[state] += 1;
        }
    })));
    for path in &args[3..] {
        counters.lock().unwrap().3 = None;
        parser
            .parse(fs::read(path)?, None)
            .context("profile parse cancelled")?;
    }
    parser.set_logger(None);
    let counters = counters.lock().unwrap();
    let profile = tree_sitter_generate::GenerationProfile {
        fingerprint: tree_sitter_generate::parser_fingerprint(&source),
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
    fs::write(output, serde_json::to_vec(&profile)?)?;
    Ok(())
}

fn measure(parser: &mut Parser, path: &str) -> Result<()> {
    let mut source = fs::read(path)?;
    let original_tree = parser.parse(&source, None).context("parse cancelled")?;
    let hash = tree_hash(&original_tree);
    let error = original_tree.root_node().has_error();
    drop(original_tree);
    let mut count = 0;
    let start = Instant::now();
    loop {
        drop(parser.parse(&source, None).context("parse cancelled")?);
        count += 1;
        if start.elapsed() >= Duration::from_millis(200) || count == 10000 {
            break;
        }
    }
    let fresh_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(count);
    let mut incremental_us = None;
    let mut incremental_result = None;
    if let Some(position) = source
        .iter()
        .enumerate()
        .skip(source.len() / 2)
        .find_map(|(i, &b)| (b == b' ' || b == b'\t').then_some(i))
    {
        let mut point = Point::new(0, 0);
        for &byte in &source[..position] {
            if byte == b'\n' {
                point.row += 1;
                point.column = 0;
            } else {
                point.column += 1;
            }
        }
        let end = Point::new(point.row, point.column + 1);
        let edit = InputEdit {
            start_byte: position,
            old_end_byte: position + 1,
            new_end_byte: position + 1,
            start_position: point,
            old_end_position: end,
            new_end_position: end,
        };
        let mut tree = parser.parse(&source, None).context("parse cancelled")?;
        count = 0;
        let start = Instant::now();
        loop {
            source[position] = if source[position] == b' ' {
                b'\t'
            } else {
                b' '
            };
            tree.edit(&edit);
            tree = parser
                .parse(&source, Some(&tree))
                .context("incremental parse cancelled")?;
            count += 1;
            if count % 2 == 0 && (start.elapsed() >= Duration::from_millis(200) || count >= 10000) {
                break;
            }
        }
        incremental_us = Some(start.elapsed().as_secs_f64() * 1e6 / f64::from(count));
        let incremental_hash = tree_hash(&tree);
        let fresh = parser.parse(&source, None).context("parse cancelled")?;
        let incremental_matches_fresh = incremental_hash == tree_hash(&fresh);
        incremental_result = Some((incremental_hash, incremental_matches_fresh));
        ensure!(
            error || incremental_matches_fresh,
            "incremental tree differs from fresh tree for valid input: {path}"
        );
    }
    println!(
        "{}",
        serde_json::json!({
            "path": path, "bytes": source.len(), "hash": format!("{hash:016x}"), "error": error,
            "fresh_us": fresh_us, "incremental_us": incremental_us,
            "incremental_hash": incremental_result.map(|(hash, _)| format!("{hash:016x}")),
            "incremental_matches_fresh": incremental_result.map(|(_, matches)| matches),
        })
    );
    Ok(())
}

fn tree_hash(tree: &Tree) -> u64 {
    let mut cursor = tree.walk();
    let mut hash = 14_695_981_039_346_656_037;
    loop {
        let node = cursor.node();
        for byte in node.kind().bytes() {
            hash = mix(hash, u32::from(byte));
        }
        for value in [
            node.start_byte(),
            node.end_byte(),
            node.start_position().row,
            node.start_position().column,
            node.end_position().row,
            node.end_position().column,
        ] {
            hash = mix(hash, value as u32);
        }
        hash = mix(
            hash,
            u32::from(node.is_named())
                | u32::from(node.is_missing()) << 1
                | u32::from(node.is_extra()) << 2
                | u32::from(node.is_error()) << 3,
        );
        if let Some(field) = cursor.field_name() {
            for byte in field.bytes() {
                hash = mix(hash, u32::from(byte));
            }
        }
        if cursor.goto_first_child() {
            hash = mix(hash, 1);
            continue;
        }
        while !cursor.goto_next_sibling() {
            hash = mix(hash, 2);
            if !cursor.goto_parent() {
                return hash;
            }
        }
        hash = mix(hash, 3);
    }
}

fn mix(hash: u64, value: u32) -> u64 {
    (hash ^ u64::from(value)).wrapping_mul(1_099_511_628_211)
}
