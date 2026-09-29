/// <reference types="vite/client" />
import { expect, it } from 'vitest';
import { Language, Parser } from '@willbooster/web-tree-sitter';
import javascriptUrl from '../../../../target/release/tree-sitter-javascript.wasm?url';

it('parses in a browser, loading the Wasm files over HTTP', async () => {
  await Parser.init();
  const parser = new Parser();
  parser.setLanguage(await Language.load(javascriptUrl));
  expect(parser.parse('let x = 1;')?.rootNode.toString()).toBe(
    '(program (lexical_declaration (variable_declarator name: (identifier) value: (number))))',
  );
});
