import { Language, Parser, type Tree } from '@willbooster/web-tree-sitter';
import { Parser as DebugParser } from '@willbooster/web-tree-sitter/debug';

export async function parse(grammarPath: string, source: string): Promise<Tree | null> {
  await Parser.init();
  const parser = new Parser();
  parser.setLanguage(await Language.load(grammarPath));
  return parser.parse(source);
}

export async function createDebugParser(): Promise<DebugParser> {
  await DebugParser.init();
  return new DebugParser();
}
