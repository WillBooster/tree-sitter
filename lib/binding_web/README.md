# @willbooster/web-tree-sitter

[![npmjs.com badge]][npmjs.com]

[npmjs.com]: https://www.npmjs.org/package/@willbooster/web-tree-sitter
[npmjs.com badge]: https://img.shields.io/npm/v/@willbooster/web-tree-sitter.svg?color=%23BF4A4A

WebAssembly bindings to WillBooster's fork of the [Tree-sitter](https://github.com/tree-sitter/tree-sitter) parsing library.
The package runs in Node.js, Bun, browsers, and Cloudflare Workers.

## Setup

In Node.js and Bun, import the package and initialize it; it reads `web-tree-sitter.wasm` from the package:

```js
import { Parser } from '@willbooster/web-tree-sitter';
await Parser.init();
```

In browsers, bundlers select the `browser` export, which has no Node.js-specific code. The library fetches
`web-tree-sitter.wasm` next to the bundled script by default; pass `locateFile` when your bundler serves it elsewhere:

```js
import { Parser } from '@willbooster/web-tree-sitter';
import wasmUrl from '@willbooster/web-tree-sitter/web-tree-sitter.wasm?url'; // Vite

await Parser.init({ locateFile: () => wasmUrl });
```

In Cloudflare Workers, which do not allow compiling Wasm at run time, import the `.wasm` files as modules and pass
them to `Parser.init` and `Language.load`:

```js
import { Language, Parser } from '@willbooster/web-tree-sitter';
import runtime from '@willbooster/web-tree-sitter/web-tree-sitter.wasm';
import javascript from './tree-sitter-javascript.wasm';

await Parser.init({ wasmModule: runtime });
const JavaScript = await Language.load(javascript);
```

To use the debug version of the library in Node.js, import `@willbooster/web-tree-sitter/debug` instead. It loads the
debug versions of the `.js` and `.wasm` files, which include debug symbols and assertions.

### Basic Usage

First, create a parser:

```js
const parser = new Parser();
```

Then assign a language to the parser. Tree-sitter languages are packaged as individual `.wasm` files (more on this below):

```js
const { Language } = require('@willbooster/web-tree-sitter');
const JavaScript = await Language.load('/path/to/tree-sitter-javascript.wasm');
parser.setLanguage(JavaScript);
```

Now you can parse source code:

```js
const sourceCode = 'let x = 1; console.log(x);';
const tree = parser.parse(sourceCode);
```

and inspect the syntax tree.

```javascript
console.log(tree.rootNode.toString());

// (program
//   (lexical_declaration
//     (variable_declarator name: (identifier) value: (number)))
//   (expression_statement
//     (call_expression
//       function: (member_expression object: (identifier) property: (property_identifier))
//       arguments: (arguments (identifier)))))
// (printed on one line)

const callExpression = tree.rootNode.child(1).firstChild;
console.log(callExpression.type, callExpression.startPosition, callExpression.endPosition);

// call_expression { row: 0, column: 11 } { row: 0, column: 25 }
```

### Editing

If your source code *changes*, you can update the syntax tree. This will take less time than the first parse.

```javascript
// Replace 'let' with 'const'
const newSourceCode = 'const x = 1; console.log(x);';

tree.edit({
  startIndex: 0,
  oldEndIndex: 3,
  newEndIndex: 5,
  startPosition: {row: 0, column: 0},
  oldEndPosition: {row: 0, column: 3},
  newEndPosition: {row: 0, column: 5},
});

const newTree = parser.parse(newSourceCode, tree);
```

### Parsing Text From a Custom Data Structure

If your text is stored in a data structure other than a single string, you can parse it by supplying a callback to `parse`
instead of a string:

```javascript
const sourceLines = [
  'let x = 1;',
  'console.log(x);'
];

const tree = parser.parse((index, position) => {
  let line = sourceLines[position.row];
  if (line) return line.slice(position.column);
});
```

### Getting the `.wasm` language files

There are several options on how to get the `.wasm` files for the languages you want to parse.

#### From npmjs.com

WillBooster's grammar packages ship their `.wasm` files: `@willbooster/tree-sitter-bash`, `-c`, `-cpp`, `-c-sharp`,
`-javascript`, `-kotlin`, `-rust`, and `-typescript`. For example, to parse JavaScript:

```sh
npm install @willbooster/tree-sitter-javascript
```

Then you can find `tree-sitter-javascript.wasm` in the `node_modules/@willbooster/tree-sitter-javascript` directory.
Other grammar packages on npm, such as `tree-sitter-javascript`, may ship a `.wasm` file as well.

#### From GitHub

You can also download the `.wasm` files from the GitHub releases of grammars that publish them with Tree-sitter's
reusable workflow, such as the tree-sitter-javascript [releases page][gh release js].

#### Generating `.wasm` files

You can also generate the `.wasm` file for your desired grammar with the CLI that each
[release of this repository][releases] carries as `tree-sitter-cli-<platform>.tar.gz` (`linux-x64`, `linux-arm64`,
`macos-arm64`, or `macos-x64`), which extracts to an executable `tree-sitter`. `tree-sitter build --wasm` downloads
[wasi-sdk][] on first use, so no other tools need to be installed. For example, for the JavaScript grammar:

```sh
curl -fsSL https://github.com/WillBooster/tree-sitter/releases/latest/download/tree-sitter-cli-macos-arm64.tar.gz | tar -xz
npm install tree-sitter-javascript
./tree-sitter build --wasm node_modules/tree-sitter-javascript
```

If everything is fine, file `tree-sitter-javascript.wasm` should be generated in current directory.

### Wasm compatibility

`@willbooster/web-tree-sitter` loads parsers of ABI versions 13 to 15.

> [!WARNING]
> Some prebuilt `.wasm` files use an older dynamic-linking format that newer versions of `web-tree-sitter` cannot
> load, even if their parser ABI is supported. Rebuild these files using a current tree-sitter CLI.

### Running .wasm in Node.js

Notice that executing `.wasm` files in Node.js is considerably slower than running [Node.js bindings][node bindings].
However, this could be useful for testing purposes:

```javascript
import { Language, Parser } from '@willbooster/web-tree-sitter';

await Parser.init();
const parser = new Parser();
parser.setLanguage(await Language.load('tree-sitter-javascript.wasm'));
const tree = parser.parse('let x = 1;');
console.log(tree.rootNode.toString());
```

### Loading a pre-compiled WebAssembly module

Some environments, such as Cloudflare Workers and Vercel Edge Functions, import
`.wasm` files as `WebAssembly.Module` objects. `Language.load` accepts those modules, and `Language.loadSync` loads them
synchronously:

```javascript
import treeSitterJavaScript from 'tree-sitter-javascript.wasm';
// treeSitterJavaScript is of type `WebAssembly.Module`
const JavaScript = Language.loadSync(treeSitterJavaScript);
parser.setLanguage(JavaScript);
```

### Running .wasm in browser

`@willbooster/web-tree-sitter` can run in the browser, but there are some common pitfalls.

#### Loading the .wasm file

`@willbooster/web-tree-sitter` needs to load the `web-tree-sitter.wasm` file. By default, it assumes that this file is available in the
same path as the JavaScript code. Therefore, if the code is being served from `http://localhost:3000/bundle.js`, then
the Wasm file should be at `http://localhost:3000/web-tree-sitter.wasm`.

For server side frameworks like NextJS, this can be tricky as pages are often served from a path such as
`http://localhost:3000/_next/static/chunks/pages/index.js`. The loader will therefore look for the Wasm file at
`http://localhost:3000/_next/static/chunks/pages/web-tree-sitter.wasm`. The solution is to pass a `locateFile` function in
the `moduleOptions` argument to `Parser.init()`:

```javascript
await Parser.init({
  locateFile(scriptName: string, scriptDirectory: string) {
    return scriptName;
  },
});
```

`locateFile` takes in two parameters, `scriptName`, i.e. the Wasm file name, and `scriptDirectory`, i.e. the directory
where the loader expects the script to be. It returns the path where the loader will look for the Wasm file. In the NextJS
case, we want to return just the `scriptName` so that the loader will look at `http://localhost:3000/web-tree-sitter.wasm`
and not `http://localhost:3000/_next/static/chunks/pages/web-tree-sitter.wasm`.

For more information on the module options you can pass in, see the [emscripten documentation][emscripten-module-options].

[emscripten-module-options]: https://emscripten.org/docs/api_reference/module.html#affecting-execution
[gh release js]: https://github.com/tree-sitter/tree-sitter-javascript/releases/latest
[releases]: https://github.com/WillBooster/tree-sitter/releases
[node bindings]: https://github.com/tree-sitter/node-tree-sitter
[wasi-sdk]: https://github.com/WebAssembly/wasi-sdk
