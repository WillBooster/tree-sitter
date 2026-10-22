#include "tree_sitter/parser.h"

void *tree_sitter_column_replay_external_scanner_create(void) { return NULL; }
void tree_sitter_column_replay_external_scanner_destroy(void *payload) {}
unsigned tree_sitter_column_replay_external_scanner_serialize(void *payload, char *buffer) { return 0; }
void tree_sitter_column_replay_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {}

static bool is_gap(int32_t character) {
  return character == ' ' || character == '\r' || character == '\n' ||
    character == '\t' || character == '\f' || character == 0x3bb || character == 0x1f600;
}

bool tree_sitter_column_replay_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid) {
  while (is_gap(lexer->lookahead)) lexer->advance(lexer, true);
  if (lexer->lookahead != 'x') return false;
  uint32_t column = lexer->get_column(lexer);
  lexer->advance(lexer, false);
  lexer->mark_end(lexer);
  while (is_gap(lexer->lookahead)) lexer->advance(lexer, false);
  lexer->get_column(lexer);
  lexer->result_symbol = column % 2;
  return true;
}
