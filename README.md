# WillBooster/tree-sitter

[![wbfy](https://img.shields.io/badge/wbfy-20.24.0-1e90ff.svg)](https://github.com/WillBooster/shared/tree/main/packages/wbfy)

This is a fork of [tree-sitter/tree-sitter](https://github.com/tree-sitter/tree-sitter). We are grateful to its authors and
contributors. This is not an official release of that project.

This fork provides the Tree-sitter parsing library for WillBooster's products in two forms, and fixes the bugs they run
into:

- the Rust crate [`willbooster-tree-sitter`](lib/binding_rust/README.md), used by code-gauge and ultra-uni, natively and
  compiled to Wasm;
- the npm package [`@willbooster/web-tree-sitter`](lib/binding_web/README.md), which runs in browsers and in Cloudflare
  Workers.

The CLI and the parser generator under `crates/` build the test fixtures and are not published as packages. The GitHub
Releases from v1.0.7 on carry the CLI as `tree-sitter-cli-<platform>.tar.gz` for `linux-x64`, `linux-arm64`,
`macos-arm64`, and `macos-x64`, so that the grammar repositories can generate their parsers with this generator and run
`tree-sitter fuzz` on this runtime without building it. A workflow attaches the archives after a release is published,
so the newest release lacks them for a while.

Tree-sitter is a parser generator tool and an incremental parsing library. It can build a concrete syntax tree for a source file and efficiently update the syntax tree as the source file is edited. Tree-sitter aims to be:

- **General** enough to parse any programming language
- **Fast** enough to parse on every keystroke in a text editor
- **Robust** enough to provide useful results even in the presence of syntax errors
- **Dependency-free** so that the runtime library (which is written in pure C) can be embedded in any application

## Links

- [Tree-sitter documentation](https://tree-sitter.github.io) (upstream)
- [Rust binding](lib/binding_rust/README.md)
- [Wasm binding](lib/binding_web/README.md)
