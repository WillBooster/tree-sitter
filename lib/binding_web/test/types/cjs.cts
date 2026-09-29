import { Language, Parser } from '@willbooster/web-tree-sitter';

export function createParser(): Parser {
  return new Parser();
}

export const LanguageClass: typeof Language = Language;
