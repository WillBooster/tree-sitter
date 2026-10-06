# WillBooster/tree-sitter

[![Test](https://github.com/WillBooster/tree-sitter/actions/workflows/test.yml/badge.svg)](https://github.com/WillBooster/tree-sitter/actions/workflows/test.yml)
[![Test rust](https://github.com/WillBooster/tree-sitter/actions/workflows/test-rust.yml/badge.svg)](https://github.com/WillBooster/tree-sitter/actions/workflows/test-rust.yml)
[![semantic-release](https://img.shields.io/badge/%20%20%F0%9F%93%A6%F0%9F%9A%80-semantic--release-e10079.svg)](https://github.com/semantic-release/semantic-release)
[![wbfy](https://img.shields.io/badge/wbfy-20.28.7-1e90ff.svg)](https://github.com/WillBooster/shared/tree/main/packages/wbfy)

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

## Parser generation and tuning

The CLI defaults to ABI 15. Use `tree-sitter generate --abi 16` for shared parse rows, bitmap-ranked sparse lookup, and compact metadata. These parsers require a runtime supporting ABI 16.

To record parse-action and lexer-entry frequencies, generate an unprofiled ABI 16 parser first. From this repository's checkout, run the measurement example on representative grammar inputs:

```sh
mise exec -- cargo run --release -p tree-sitter-cli --features wasm --example parser_study -- \
  --profile /path/to/grammar/profile.json native bash /path/to/grammar/src /path/to/training.sh
```

Then, from the grammar directory:

```sh
tree-sitter generate --abi 16 --profile profile.json
tree-sitter build
```

A profile applies only to the exact unprofiled generated source it was recorded from. Regenerate and record again after changing the grammar, generation options, or generator version. Compare fresh and incremental parsing on held-out valid and malformed inputs before choosing the profile-guided layout. The example also supports `wasm` with a module path in place of the source directory. When recording a Wasm profile, that module's directory must contain `src/parser.c` from the exact unprofiled generation used to build the module.

`build --optimization` selects `2`, `3`, `s`, or `z` for native or Wasm builds. The defaults are native `2` and Wasm `s`. Compare build time, artifact size, and parsing time for the target compiler and runtime.

Tree-sitter is a parser generator tool and an incremental parsing library. It can build a concrete syntax tree for a source file and efficiently update the syntax tree as the source file is edited. Tree-sitter aims to be:

- **General** enough to parse any programming language
- **Fast** enough to parse on every keystroke in a text editor
- **Robust** enough to provide useful results even in the presence of syntax errors
- **Dependency-free** so that the runtime library (which is written in pure C) can be embedded in any application

## Restricted rules with canonical supertype queries

This fork supports aliasing one declared supertype to another while keeping both hidden in the syntax tree. This lets a
restricted grammar context retain queries such as `(expression/identifier)` without introducing a visible wrapper node.
Generate the parser and run its queries with this fork's generator and runtime.

Declare both rules in `supertypes`, use the same named alias at every reference to the restricted rule, and leave the
canonical supertype unaliased. Reference the canonical rule as a symbol in a reachable production, as `$.expression`
in the general branch below does. A rule named only as an alias target is removed as unused, leaving an ordinary visible
alias instead of a transparent supertype. The generated node schema combines the retained supertypes under the canonical
name. Concrete-node aliases and inconsistent aliases still cannot reuse a canonical supertype's name.

```js
supertypes: $ => [$.expression, $._restricted_expression],
rules: {
  source_file: $ => choice(
    seq('general', $.expression),
    seq('restricted', alias($._restricted_expression, $.expression)),
  ),
  expression: $ => choice($.identifier, $.number),
  _restricted_expression: $ => $.identifier,
  identifier: () => /[a-z]+/,
  number: () => /[0-9]+/,
},
```

## Links

- [Tree-sitter documentation](https://tree-sitter.github.io) (upstream)
- [Rust binding](lib/binding_rust/README.md)
- [Wasm binding](lib/binding_web/README.md)
