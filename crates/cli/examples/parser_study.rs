use std::{
    env, fs,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use tree_sitter::{Parser, Tree};
use tree_sitter_loader::{CompileConfig, Loader};

fn main() -> Result<()> {
    tree_sitter_cli::logger::init();
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
        "usage: cargo run --release -p tree-sitter-cli --features wasm --example parser_study -- [--profile <output.json>] <native|wasm> <language> <src-directory|wasm-file> <input>..."
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
            anyhow::bail!(
                "Wasm measurements require cargo run --release -p tree-sitter-cli --features wasm --example parser_study -- wasm <language> <wasm-file> <input>..."
            )
        }
        backend => anyhow::bail!("unsupported backend: {backend}"),
    };
    parser.set_language(&language)?;
    #[cfg(debug_assertions)]
    eprintln!(
        "parser_study: debug assertions are enabled; use cargo run --release for comparable performance measurements"
    );
    for path in &args[3..] {
        measure(&mut parser, path)?;
    }
    if let Some(path) = profile_path {
        record_profile(&mut parser, &args, &path)?;
    }
    Ok(())
}

fn record_profile(parser: &mut Parser, args: &[String], output: &str) -> Result<()> {
    let parser_path = if args[0] == "native" {
        Path::new(&args[2]).join("parser.c")
    } else {
        Path::new(&args[2])
            .parent()
            .context("Wasm file must have a parent")?
            .join("src/parser.c")
    };
    let source = fs::read_to_string(parser_path)?;
    let inputs = args[3..]
        .iter()
        .map(fs::read)
        .collect::<std::io::Result<Vec<_>>>()?;
    let profile = tree_sitter_cli::generation_profile::record_profile(parser, &source, &inputs)?;
    fs::write(output, serde_json::to_vec(&profile)?)?;
    Ok(())
}

fn measure(parser: &mut Parser, path: &str) -> Result<()> {
    let source = fs::read(path)?;
    let original_tree = parser.parse(&source).context("parse cancelled")?;
    let hash = tree_hash(&original_tree);
    let error = original_tree.root_node().has_error();
    drop(original_tree);
    let mut count = 0;
    let start = Instant::now();
    loop {
        drop(parser.parse(&source).context("parse cancelled")?);
        count += 1;
        if start.elapsed() >= Duration::from_millis(200) || count == 10000 {
            break;
        }
    }
    let parse_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(count);
    println!(
        "{}",
        serde_json::json!({
            "path": path,
            "bytes": source.len(),
            "hash": format!("{hash:016x}"),
            "error": error,
            "parse_us": parse_us,
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
