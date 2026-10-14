#include "tree_sitter/parser.h"

enum { TEXT, BEGIN };

void *tree_sitter_external_token_recovery_priority_external_scanner_create(void) {
  return NULL;
}

void tree_sitter_external_token_recovery_priority_external_scanner_destroy(void *payload) {}

unsigned tree_sitter_external_token_recovery_priority_external_scanner_serialize(void *payload, char *buffer) {
  return 0;
}

void tree_sitter_external_token_recovery_priority_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {}

bool tree_sitter_external_token_recovery_priority_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  if (valid_symbols[BEGIN] && lexer->lookahead == '@') {
    lexer->result_symbol = BEGIN;
    lexer->advance(lexer, false);
    return true;
  }
  if (valid_symbols[TEXT] && lexer->lookahead == '(') {
    lexer->result_symbol = TEXT;
    do {
      lexer->advance(lexer, false);
    } while (lexer->lookahead && lexer->lookahead != ')');
    if (lexer->lookahead) lexer->advance(lexer, false);
    return true;
  }
  return false;
}
