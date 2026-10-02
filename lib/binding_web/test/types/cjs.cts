import { Language, Parser } from '@willbooster/web-tree-sitter';
import { Parser as DebugParser } from '@willbooster/web-tree-sitter/debug';
import type { DSL, GrammarSchema } from '@willbooster/web-tree-sitter/dsl';

export function defineGrammar(dsl: DSL): GrammarSchema<'source_file' | 'word'> {
  const { grammar, repeat1, RustRegex } = bindDsl(dsl);
  return grammar({
    name: 'typed_grammar',
    rules: {
      source_file: ($) => repeat1($.word),
      word: () => new RustRegex('[a-z]+'),
    },
  });
}

export function bindDsl(dsl: DSL) {
  const { alias, blank, choice, eof, field, grammar, optional, prec, repeat, repeat1, reserved, RustRegex, seq, sym, token } = dsl;
  const { left, right, dynamic } = prec;
  const { immediate } = token;
  return { alias, blank, choice, eof, field, grammar, optional, prec, repeat, repeat1, reserved, RustRegex, seq, sym, token, left, right, dynamic, immediate };
}

export function createParser(): Parser {
  return new Parser();
}

export const LanguageClass: typeof Language = Language;

export function createDebugParser(): DebugParser {
  return new DebugParser();
}
