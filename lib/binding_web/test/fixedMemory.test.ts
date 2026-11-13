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

describe('Fixed WebAssembly.Memory ceiling', () => {
  it('parses large string inputs and preserves AST headroom without allocating a second full-string copy', () => {
    const rangedTree = parser.parse('const skipped = 0; const kept = 1;', {
      includedRanges: [{
        startPosition: { row: 0, column: 19 },
        endPosition: { row: 0, column: 34 },
        startIndex: 19,
        endIndex: 34,
      }],
    });
    expect(rangedTree?.rootNode.text).toBe('const kept = 1;');
    rangedTree?.delete();

    const denseSource = 'const value = 1;\n'.repeat(26_200);
    const denseTree = parser.parse(denseSource);
    expect(denseTree?.rootNode.hasError).toBe(false);
    expect(denseTree?.rootNode.endIndex).toBe(denseSource.length);
    expect(denseTree?.rootNode.namedChildCount).toBe(26_200);
    denseTree?.delete();

    const largeLiteralSource = `const data = "${'a'.repeat(16 * 1024 * 1024 - 32)}";`;
    const literalTree = parser.parse(largeLiteralSource);
    expect(literalTree?.rootNode.hasError).toBe(false);
    expect(literalTree?.rootNode.toString()).toContain('variable_declarator');
    expect(literalTree?.rootNode.endIndex).toBe(largeLiteralSource.length);
    literalTree?.delete();

    const line = 'const value = 1;\n';
    const rangeCount = 25_600;
    const manyRangesSource = line.repeat(rangeCount);
    const includedRanges = Array.from({ length: rangeCount }, (_, i) => ({
      startIndex: i * line.length,
      endIndex: (i + 1) * line.length - 1,
      startPosition: { row: i, column: 0 },
      endPosition: { row: i, column: line.length - 1 },
    }));
    const manyRangesTree = parser.parse(manyRangesSource, { includedRanges });
    expect(manyRangesTree?.rootNode.hasError).toBe(false);
    expect(manyRangesTree?.rootNode.namedChildCount).toBe(rangeCount);
    expect(manyRangesTree?.rootNode.endIndex).toBe(manyRangesSource.length - 1);
    manyRangesTree?.delete();

    const limitedRangeCount = 24_800;
    const limitedRangesSource = line.repeat(limitedRangeCount);
    const limitedRanges = includedRanges.slice(0, limitedRangeCount);
    const limitedRangesTree = parser.parse(limitedRangesSource, {
      includedRanges: limitedRanges,
      maxMemoryBytes: 32 * 2 ** 20,
    });
    expect(limitedRangesTree?.rootNode.hasError).toBe(false);
    expect(limitedRangesTree?.rootNode.namedChildCount).toBe(limitedRangeCount);
    limitedRangesTree?.delete();
  });
});
