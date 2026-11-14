use std::{fs, process::Command};

#[test]
fn exported_playground_runtime_parses_the_generated_language() {
    let temp_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp");
    fs::create_dir_all(&temp_root).unwrap();
    let temp = tempfile::tempdir_in(temp_root).unwrap();
    fs::write(
        temp.path().join("grammar.js"),
        "module.exports = grammar({ name: 'example', rules: { source_file: $ => repeat1($.word), word: _ => /[a-z]+/ } });",
    )
    .unwrap();

    for args in [
        &["generate", "--js-runtime", "native"][..],
        &["build", "--wasm"][..],
        &["playground", "--export", "export"][..],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_tree-sitter"))
            .args(args)
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let script = r#"
import fs from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
const dir = 'export';
const html = await fs.readFile(`${dir}/index.html`, 'utf8');
const match = html.match(/import \* as TreeSitter from '([^']+)'/);
if (!match) throw new Error('Playground runtime import not found');
const url = match[1];
async function read(name) {
  if (url.startsWith('./')) return fs.readFile(`${dir}/${name}`);
  const response = await fetch(new URL(name, url));
  if (!response.ok) throw new Error(`Runtime download failed: ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
}
await fs.writeFile(`${dir}/runtime.mjs`, await read('web-tree-sitter.js'));
const runtime = await import(pathToFileURL(`${process.cwd()}/${dir}/runtime.mjs`).href);
await runtime.Parser.init({ wasmBinary: await read('web-tree-sitter.wasm') });
const language = await runtime.Language.load(new Uint8Array(await fs.readFile(`${dir}/tree-sitter-parser.wasm`)));
const parser = new runtime.Parser();
parser.setLanguage(language);
const tree = parser.parse('hello world');
if (tree.rootNode.toString() !== '(source_file (word) (word))') throw new Error(tree.rootNode.toString());
"#;
    let output = Command::new("node")
        .args(["--input-type=module", "-e", script])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
