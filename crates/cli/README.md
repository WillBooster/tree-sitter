# Tree-sitter CLI

The Tree-sitter CLI allows you to develop, test, and use Tree-sitter grammars from the command line. It works on `MacOS`,
`Linux`, and `Windows`.

### Installation

This fork's CLI is not published as a package. The [releases of this repository][the releases page] from v1.0.7 on
carry it as `tree-sitter-cli-<platform>.tar.gz` for `linux-x64`, `linux-arm64`, `macos-arm64`, and `macos-x64`, which
extracts to an executable `tree-sitter`. A workflow builds and attaches the archives after a release is published, so the
newest release lacks them for a while. On other platforms, build it from a checkout of this repository:

```sh
cargo install --locked --path crates/cli
```

### Dependencies

The `tree-sitter` binary itself has no dependencies, but specific commands have dependencies that must be present at runtime:

* To generate a parser from a grammar, you must have [`node`](https://nodejs.org) on your PATH, or pass
  `--js-runtime native` to use the bundled QuickJS runtime.
* To run and test parsers, you must have a C and C++ compiler on your system.

### Commands

* `generate` - The `tree-sitter generate` command will generate a Tree-sitter parser based on the grammar in the current
  working directory. See [the documentation] for more information.

* `test` - The `tree-sitter test` command will run the unit tests for the Tree-sitter parser in the current working directory.
  See [the documentation] for more information.

* `parse` - The `tree-sitter parse` command will parse a file (or list of files) using Tree-sitter parsers.

[the documentation]: https://tree-sitter.github.io/tree-sitter/creating-parsers
[the releases page]: https://github.com/WillBooster/tree-sitter/releases
