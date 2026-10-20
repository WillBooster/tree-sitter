#include "tree_sitter/parser.h"
#include <wctype.h>

void *tree_sitter_keyword_recovery_priority_external_scanner_create(void) { return NULL; }
void tree_sitter_keyword_recovery_priority_external_scanner_destroy(void *payload) {}
unsigned tree_sitter_keyword_recovery_priority_external_scanner_serialize(void *payload, char *buffer) { return 0; }
void tree_sitter_keyword_recovery_priority_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {}

bool tree_sitter_keyword_recovery_priority_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  if (!valid_symbols[0]) return false;
  lexer->mark_end(lexer);
  while (iswspace(lexer->lookahead)) lexer->advance(lexer, false);
  if (lexer->lookahead != ',' && lexer->lookahead != '{' && !lexer->eof(lexer)) return false;
  lexer->result_symbol = 0;
  return true;
}
