use std::{
    env, fs,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use tree_sitter::{InputEdit, Parser, Point, Tree};
use tree_sitter_loader::{CompileConfig, Loader};

fn main() -> Result<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    ensure!(
        args.len() >= 4,
        "usage: parser_study <native|wasm> <language> <src-directory|wasm-file> <input>..."
    );
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
            loader.load_language_at_path(config)?
        }
        #[cfg(feature = "wasm")]
        "wasm" => {
            let engine = tree_sitter::wasmtime::Engine::default();
            let mut store = tree_sitter::WasmStore::new(&engine)?;
            let language = store.load_language(&args[1], &fs::read(&args[2])?)?;
            parser.set_wasm_store(store)?;
            language
        }
        backend => anyhow::bail!("unsupported backend: {backend}"),
    };
    parser.set_language(&language)?;
    for path in &args[3..] {
        measure(&mut parser, path)?;
    }
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
    let mut incremental_hash = hash;
    let mut incremental_matches_fresh = true;
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
        incremental_hash = tree_hash(&tree);
        let fresh = parser.parse(&source, None).context("parse cancelled")?;
        incremental_matches_fresh = incremental_hash == tree_hash(&fresh);
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
            "incremental_hash": format!("{incremental_hash:016x}"),
            "incremental_matches_fresh": incremental_matches_fresh,
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
