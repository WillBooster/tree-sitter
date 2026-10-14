/// <reference types="vite/client" />
import { expect, it } from 'vitest';
import { Language, Parser } from '@willbooster/web-tree-sitter';
import javascriptUrl from '../../../../target/release/tree-sitter-javascript.wasm?url';

it.each(['URL', 'module'] as const)('loads a grammar larger than 8 MiB from a %s', async (kind) => {
  await Parser.init();
  const response = await fetch(javascriptUrl);
  const binary = oversizedGrammar(new Uint8Array(await response.arrayBuffer()));
  const url = URL.createObjectURL(new Blob([binary], { type: 'application/wasm' }));
  const parser = new Parser();
  try {
    const input = kind === 'URL' ? url : await WebAssembly.compile(binary);
    parser.setLanguage(await Language.load(input));
    const tree = parser.parse('let x = 1;');
    try {
      expect(tree?.rootNode.toString()).toBe(
        '(program (lexical_declaration (variable_declarator name: (identifier) value: (number))))',
      );
    } finally {
      tree?.delete();
    }
  } finally {
    parser.delete();
    URL.revokeObjectURL(url);
  }
});

function oversizedGrammar(original: Uint8Array) {
  const payload = new Uint8Array(9 * 1024 * 1024);
  const name = new TextEncoder().encode('padding');
  payload.set([name.length, ...name]);
  const size = [];
  for (let value = payload.length; value > 0; value >>>= 7) {
    size.push((value & 0x7f) | (value > 0x7f ? 0x80 : 0));
  }
  const binary = new Uint8Array(original.length + 1 + size.length + payload.length);
  binary.set(original);
  binary.set([0, ...size], original.length);
  binary.set(payload, original.length + 1 + size.length);
  return binary;
}
