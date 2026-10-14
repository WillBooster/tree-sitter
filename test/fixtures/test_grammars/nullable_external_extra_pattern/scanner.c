#include "tree_sitter/parser.h"
#include <wctype.h>

void *
tree_sitter_nullable_external_extra_pattern_external_scanner_create(void) {
  return NULL;
}
void tree_sitter_nullable_external_extra_pattern_external_scanner_destroy(
    void *payload) {}
unsigned tree_sitter_nullable_external_extra_pattern_external_scanner_serialize(
    void *payload, char *buffer) {
  return 0;
}
void tree_sitter_nullable_external_extra_pattern_external_scanner_deserialize(
    void *payload, const char *buffer, unsigned length) {}

bool tree_sitter_nullable_external_extra_pattern_external_scanner_scan(
    void *payload, TSLexer *lexer, const bool *valid_symbols) {
  if (!valid_symbols[0] || !iswspace(lexer->lookahead))
    return false;
  lexer->advance(lexer, false);
  lexer->result_symbol = 0;
  return true;
}
