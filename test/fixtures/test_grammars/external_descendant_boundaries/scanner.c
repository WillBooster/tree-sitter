#include "tree_sitter/parser.h"

void *tree_sitter_external_descendant_boundaries_external_scanner_create(void) { return NULL; }
void tree_sitter_external_descendant_boundaries_external_scanner_destroy(void *payload) {}
unsigned tree_sitter_external_descendant_boundaries_external_scanner_serialize(void *payload, char *buffer) { return 0; }
void tree_sitter_external_descendant_boundaries_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {}

bool tree_sitter_external_descendant_boundaries_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  if (lexer->lookahead != ' ') return false;
  while (lexer->lookahead == ' ') lexer->advance(lexer, false);
  lexer->mark_end(lexer);
  if (lexer->lookahead != ')') return false;
  lexer->advance(lexer, false);
  if (lexer->lookahead == 'a' && valid_symbols[0]) {
    lexer->result_symbol = 0;
    return true;
  }
  if (lexer->lookahead == 'b' && valid_symbols[1]) {
    lexer->result_symbol = 1;
    return true;
  }
  return false;
}
