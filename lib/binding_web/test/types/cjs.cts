import { Language, Parser } from '@willbooster/web-tree-sitter';
import { Parser as DebugParser } from '@willbooster/web-tree-sitter/debug';
import type { DSL, GrammarSchema } from '@willbooster/web-tree-sitter/dsl';

export function defineGrammar(dsl: DSL): GrammarSchema<'source_file' | 'word'> {
  const { grammar, repeat1, RustRegex } = dsl;
  return grammar({
    name: 'typed_grammar',
    rules: {
      source_file: ($) => repeat1($.word),
      word: () => new RustRegex('[a-z]+'),
    },
  });
}

export function createParser(): Parser {
  return new Parser();
}

export const LanguageClass: typeof Language = Language;

export function createDebugParser(): DebugParser {
  return new DebugParser();
}
