import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { afterEach, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { Language, Parser } from '../src';

const resolve = createRequire(import.meta.url).resolve;
const load = (specifier: string) => new WebAssembly.Module(readFileSync(resolve(specifier)));

let typescript: Language;
let parser: Parser;

beforeAll(async () => {
  const memory = new WebAssembly.Memory({ initial: 512, maximum: 512 });
  await Parser.init({
    wasmModule: load('@willbooster/web-tree-sitter/web-tree-sitter.wasm'),
    wasmMemory: memory,
  });
  typescript = Language.loadSync(load('@willbooster/tree-sitter-typescript/tree-sitter-typescript.wasm'));
});

beforeEach(() => {
  parser = new Parser();
  parser.setLanguage(typescript);
});

afterEach(() => {
  parser.delete();
});

describe('Fixed WebAssembly.Memory fallback', () => {
  it('falls back to chunked parsing when the contiguous string buffer cannot be allocated', () => {
    const rangedTree = parser.parse('const skipped = 0; const kept = 1;', undefined, {
      includedRanges: [{
        startPosition: { row: 0, column: 19 },
        endPosition: { row: 0, column: 34 },
        startIndex: 19,
        endIndex: 34,
      }],
    });
    expect(rangedTree?.rootNode.text).toBe('const kept = 1;');
    rangedTree?.delete();

    const source = `const data = "${'a'.repeat(16 * 1024 * 1024 - 32)}";`;
    const tree = parser.parse(source);
    expect(tree?.rootNode.hasError).toBe(false);
    expect(tree?.rootNode.toString()).toContain('variable_declarator');
    expect(tree?.rootNode.endIndex).toBe(source.length);
    tree?.delete();
  });
});
