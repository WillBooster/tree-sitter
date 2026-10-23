#include "tree_sitter/parser.h"

void *tree_sitter_column_keyword_rewind_external_scanner_create(void) { return NULL; }
void tree_sitter_column_keyword_rewind_external_scanner_destroy(void *payload) {}
unsigned tree_sitter_column_keyword_rewind_external_scanner_serialize(void *payload, char *buffer) { return 0; }
void tree_sitter_column_keyword_rewind_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {}

bool tree_sitter_column_keyword_rewind_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid) {
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') lexer->advance(lexer, true);
  if (lexer->lookahead != '\r' && lexer->lookahead != '\n') return false;
  if (valid[0]) {
    lexer->mark_end(lexer);
    lexer->advance(lexer, true);
    lexer->result_symbol = 0;
    return true;
  }
  if (!valid[1]) return false;
  lexer->advance(lexer, true);
  lexer->mark_end(lexer);
  if (lexer->lookahead == 'd') lexer->get_display_column(lexer, 4);
  lexer->get_column(lexer);
  if (lexer->lookahead == 's') lexer->get_display_column(lexer, 4);
  lexer->result_symbol = 1;
  return true;
}
