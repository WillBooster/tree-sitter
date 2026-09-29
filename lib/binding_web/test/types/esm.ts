import { Language, Parser, type Tree } from '@willbooster/web-tree-sitter';
import { Parser as DebugParser } from '@willbooster/web-tree-sitter/debug';

// Values cross between the CommonJS and the ES module declarations of the same types.
import { LanguageClass, createParser } from './cjs.cjs';

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
