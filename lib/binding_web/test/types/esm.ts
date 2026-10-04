import { Language, Parser, type Tree } from '@willbooster/web-tree-sitter';
import { Parser as DebugParser } from '@willbooster/web-tree-sitter/debug';
import type { DSL, GrammarSchema } from '@willbooster/web-tree-sitter/dsl';

// Values cross between the CommonJS and the ES module declarations of the same types.
import { LanguageClass, createParser, defineGrammar } from './cjs.cjs';

export function defineEsmGrammar(dsl: DSL): GrammarSchema<'source_file' | 'word'> {
  return defineGrammar(dsl);
}

export async function parse(grammarPath: string, source: string): Promise<Tree | null> {
  await Parser.init();
  const parser: Parser = createParser();
  const LoadedLanguage: typeof Language = LanguageClass;
  parser.setLanguage(await LoadedLanguage.load(grammarPath));
  return parser.parse(source);
}

export async function createDebugParser(): Promise<DebugParser> {
  await DebugParser.init();
  return new DebugParser();
}
