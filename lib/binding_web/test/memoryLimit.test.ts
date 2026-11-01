import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { beforeAll, beforeEach, afterEach, describe, expect, it } from 'vitest';
import { Language, Parser } from '../src';

const resolve = createRequire(import.meta.url).resolve;
const load = (specifier: string) => new WebAssembly.Module(readFileSync(resolve(specifier)));

let memory: WebAssembly.Memory;
let typescript: Language;
let html: Language;
let parser: Parser;

beforeAll(async () => {
  memory = new WebAssembly.Memory({ initial: 512, maximum: 1024 });
  await Parser.init({
    wasmModule: load('@willbooster/web-tree-sitter/web-tree-sitter.wasm'),
    wasmMemory: memory,
  });
  typescript = Language.loadSync(load('@willbooster/tree-sitter-typescript/tree-sitter-typescript.wasm'));
  html = Language.loadSync(load(new URL('../../../target/release/tree-sitter-html.wasm', import.meta.url).pathname));
});

beforeEach(() => {
  parser = new Parser();
  parser.setLanguage(typescript);
});

afterEach(() => {
  parser.delete();
});

describe('Parse memory limit', () => {
  it('stays within the limit and leaves the parser usable after repeated cancellations', () => {
    const maxMemoryBytes = 48 * 2 ** 20;
    const source = 'f<'.repeat(50_000);
    for (let i = 0; i < 2; i++) {
      expect(parser.parse(source, undefined, { maxMemoryBytes })).toBeNull();
      expect(memory.buffer.byteLength).toBeLessThanOrEqual(maxMemoryBytes);
      const tree = parser.parse('const value = 1;', undefined, { maxMemoryBytes });
      expect(tree?.rootNode.hasError).toBe(false);
      expect(tree?.rootNode.toString()).toContain('variable_declarator');
      tree?.delete();
      const ordinaryTree = parser.parse('const value = 2;');
      expect(ordinaryTree?.rootNode.hasError).toBe(false);
      ordinaryTree?.delete();
    }
  });

  it('cleans up when a callback throws during a limited parse', () => {
    expect(() => parser.parse('f<'.repeat(1000), undefined, {
      maxMemoryBytes: 48 * 2 ** 20,
      progressCallback: () => { throw new Error('callback failed'); },
    })).toThrow('callback failed');
    const tree = parser.parse('const value = 1;', undefined, { maxMemoryBytes: 48 * 2 ** 20 });
    expect(tree?.rootNode.hasError).toBe(false);
    tree?.delete();
  });

  it('reports the included ranges after a limited parse', () => {
    const source = 'const first = 1; const second = 2;';
    const startIndex = source.indexOf('const second');
    const range = {
      startPosition: { row: 0, column: startIndex },
      endPosition: { row: 0, column: source.length },
      startIndex,
      endIndex: source.length,
    };
    const tree = parser.parse(source, undefined, {
      includedRanges: [range],
      maxMemoryBytes: 48 * 2 ** 20,
    });
    expect(tree?.rootNode.hasError).toBe(false);
    expect(parser.getIncludedRanges()).toEqual([range]);
    expect(tree?.getIncludedRanges()).toEqual([range]);
    tree?.delete();
  });

  it('applies the limit before copying included ranges into Wasm', () => {
    const currentBytes = memory.buffer.byteLength;
    const range = {
      startPosition: { row: 0, column: 0 },
      endPosition: { row: 0, column: 1 },
      startIndex: 0,
      endIndex: 1,
    };
    const includedRanges = new Array<typeof range>(2_000_000).fill(range);
    expect(parser.parse('x', undefined, { includedRanges, maxMemoryBytes: currentBytes })).toBeNull();
    expect(memory.buffer.byteLength).toBe(currentBytes);
    const tree = parser.parse('const value = 1;');
    expect(tree?.rootNode.hasError).toBe(false);
    tree?.delete();
  });

  it('recovers when an allocating external scanner reaches the limit', () => {
    parser.setLanguage(html);
    const maxMemoryBytes = memory.buffer.byteLength + 2 * 2 ** 20;
    expect(parser.parse('<div>'.repeat(60_000), undefined, { maxMemoryBytes })).toBeNull();
    expect(memory.buffer.byteLength).toBeLessThanOrEqual(maxMemoryBytes);
    const tree = parser.parse('<div>ok</div>');
    expect(tree?.rootNode.hasError).toBe(false);
    tree?.delete();
  });
});
