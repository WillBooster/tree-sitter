import { Language, Parser } from '@willbooster/web-tree-sitter';
import { Parser as DebugParser } from '@willbooster/web-tree-sitter/debug';

export function createParser(): Parser {
  return new Parser();
}

export const LanguageClass: typeof Language = Language;

export function createDebugParser(): DebugParser {
  return new DebugParser();
}
