import path from 'path';
import { createTestHarness, type TestHarness } from 'wrangler';
import { afterAll, beforeAll, expect, it } from 'vitest';

// The same Worker with and without Node.js compatibility, since the package must run in both.
const configs = {
  'web-tree-sitter-test': 'wrangler.jsonc',
  'web-tree-sitter-test-no-nodejs-compat': 'wrangler.no-nodejs-compat.jsonc',
};

let server: TestHarness | undefined;

beforeAll(async () => {
  server = createTestHarness({
    workers: Object.values(configs).map((config) => ({ configPath: path.join(import.meta.dirname, 'workerd', config) })),
  });
  await server.listen();
}, 120_000);

afterAll(async () => {
  await server?.close();
});

it.each(Object.keys(configs))('parses in Cloudflare Workers with the imported Wasm modules (%s)', async (name) => {
  const response = await server!.getWorker(name).fetch('http://localhost/', { method: 'POST', body: 'let x = 1;' });
  expect(await response.text()).toBe(
    '(program (lexical_declaration (variable_declarator name: (identifier) value: (number))))',
  );
});
