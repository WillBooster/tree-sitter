import path from 'path';
import { createTestHarness, type TestHarness } from 'wrangler';
import { afterAll, beforeAll, expect, it } from 'vitest';

let server: TestHarness | undefined;

beforeAll(async () => {
  server = createTestHarness({ workers: [{ configPath: path.join(import.meta.dirname, 'workerd', 'wrangler.jsonc') }] });
  await server.listen();
}, 120_000);

afterAll(async () => {
  await server?.close();
});

it('parses in Cloudflare Workers with the imported Wasm modules', async () => {
  const response = await server!.getWorker().fetch('http://localhost/', { method: 'POST', body: 'let x = 1;' });
  expect(await response.text()).toBe(
    '(program (lexical_declaration (variable_declarator name: (identifier) value: (number))))',
  );
});
