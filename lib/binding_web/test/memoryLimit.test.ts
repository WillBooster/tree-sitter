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
  memory = new WebAssembly.Memory({ initial: 512, maximum: 2048 });
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
  it('does not allocate for excluded text when parsing sparse included ranges', () => {
    const firstFragment = 'const first = 1; ';
    const secondFragment = 'const second = 2;';
    const filler = ' '.repeat(16 * 1024 * 1024 - firstFragment.length - secondFragment.length);
    const source = firstFragment + filler + secondFragment;
    const secondStart = firstFragment.length + filler.length;
    const beforeBytes = memory.buffer.byteLength;

    const singleRangeTree = parser.parse(source, undefined, {
      includedRanges: [{
        startPosition: { row: 0, column: secondStart },
        endPosition: { row: 0, column: source.length },
        startIndex: secondStart,
        endIndex: source.length,
      }],
    });
    expect(singleRangeTree?.rootNode.hasError).toBe(false);
    expect(singleRangeTree?.rootNode.text).toBe(secondFragment);
    expect(memory.buffer.byteLength).toBe(beforeBytes);
    singleRangeTree?.delete();

    const multiRangeTree = parser.parse(source, undefined, {
      includedRanges: [
        {
          startPosition: { row: 0, column: 0 },
          endPosition: { row: 0, column: firstFragment.length },
          startIndex: 0,
          endIndex: firstFragment.length,
        },
        {
          startPosition: { row: 0, column: secondStart },
          endPosition: { row: 0, column: source.length },
          startIndex: secondStart,
          endIndex: source.length,
        },
      ],
    });
    expect(multiRangeTree?.rootNode.hasError).toBe(false);
    expect(multiRangeTree?.rootNode.childCount).toBe(2);
    expect(memory.buffer.byteLength).toBe(beforeBytes);
    multiRangeTree?.delete();

    const emptyRangeListTree = parser.parse('const third = 3;', undefined, { includedRanges: [] });
    expect(emptyRangeListTree?.rootNode.text).toBe('const third = 3;');
    emptyRangeListTree?.delete();
  });

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
      includedRanges: [{
        startPosition: { row: 0, column: 0 },
        endPosition: { row: 0, column: 2000 },
        startIndex: 0,
        endIndex: 2000,
      }],
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

  it('resets included ranges on subsequent parses after cancelled limited parses', () => {
    const prefix = 'const first = 1; ';
    const source = prefix + 'const second = 2;\n'.repeat(500);
    const startIndex = prefix.length;
    const range = {
      startPosition: { row: 0, column: startIndex },
      endPosition: { row: 500, column: 0 },
      startIndex,
      endIndex: source.length,
    };
    const maxMemoryBytes = 48 * 2 ** 20;

    expect(parser.parse(source, undefined, {
      includedRanges: [range],
      maxMemoryBytes,
      progressCallback: () => true,
    })).toBeNull();
    expect(parser.getIncludedRanges()).toEqual([range]);

    const fullTree = parser.parse('const value = 1;');
    expect(fullTree?.rootNode.toString()).toContain('variable_declarator');
    expect(fullTree?.rootNode.text).toBe('const value = 1;');
    fullTree?.delete();

    const rangedTree = parser.parse('const first = 1; const second = 2;', undefined, {
      includedRanges: [{
        startPosition: { row: 0, column: startIndex },
        endPosition: { row: 0, column: 34 },
        startIndex,
        endIndex: 34,
      }],
    });
    expect(rangedTree?.rootNode.text).toBe('const second = 2;');
    rangedTree?.delete();

    expect(parser.parse(source, undefined, {
      maxMemoryBytes,
      progressCallback: () => true,
    })).toBeNull();
    expect(parser.getIncludedRanges()).toHaveLength(1);
    expect(parser.getIncludedRanges()[0].startIndex).toBe(0);

    const callbackTree = parser.parse((index) => index === 0 ? 'const after = 3;' : '');
    expect(callbackTree?.rootNode.text).toBe('const after = 3;');
    callbackTree?.delete();
  });

  it('applies the limit before copying included ranges into Wasm', () => {
    const currentBytes = memory.buffer.byteLength;
    const range = {
      startPosition: { row: 0, column: 0 },
      endPosition: { row: 0, column: 1 },
      startIndex: 0,
      endIndex: 1,
    };
    const includedRanges = new Array<typeof range>(4_000_000).fill(range);
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
  }, 30_000);
});
