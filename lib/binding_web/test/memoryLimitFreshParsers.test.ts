import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { expect, it } from 'vitest';
import { Language, Parser } from '../src';

const resolve = createRequire(import.meta.url).resolve;
const load = (path: string | URL) => new WebAssembly.Module(readFileSync(path));

// A file of its own: it needs a fresh, small memory, and it fills the heap with parsers that keep their caches.
it('returns null instead of trapping when a fresh parser cannot reserve its cache under the limit', async () => {
  const memory = new WebAssembly.Memory({ initial: 256, maximum: 2048 });
  await Parser.init({ wasmModule: load(resolve('@willbooster/web-tree-sitter/web-tree-sitter.wasm')), wasmMemory: memory });
  const bash = Language.loadSync(load(new URL('../../../target/release/tree-sitter-bash.wasm', import.meta.url)));
  const parsers: Parser[] = [];
  const outcomes = new Set<string>();
  try {
    for (let i = 0; i < 400; i++) {
      const parser = new Parser();
      parser.setLanguage(bash);
      parsers.push(parser);
      const tree = parser.parse('echo hello\n', { maxMemoryBytes: memory.buffer.byteLength });
      outcomes.add(tree ? 'tree' : 'null');
      tree?.delete();
    }
  } finally {
    for (const parser of parsers) parser.delete();
  }
  expect(outcomes).toContain('tree');
}, 120_000);
