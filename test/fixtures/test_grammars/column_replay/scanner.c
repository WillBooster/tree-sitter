#include "tree_sitter/parser.h"

void *tree_sitter_column_replay_external_scanner_create(void) { return NULL; }
void tree_sitter_column_replay_external_scanner_destroy(void *payload) {}
unsigned tree_sitter_column_replay_external_scanner_serialize(void *payload, char *buffer) { return 0; }
void tree_sitter_column_replay_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {}

static bool is_gap(int32_t character) {
  return character == ' ' || character == '\r' || character == '\n' ||
    character == '\t' || character == '\f' || character == 0x3bb || character == 0x1f600;
}

static uint32_t query_column(TSLexer *lexer, int32_t mode);

bool tree_sitter_column_replay_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid) {
  while (is_gap(lexer->lookahead)) lexer->advance(lexer, true);
  int32_t mode = lexer->lookahead;
  if (mode == 'a' || mode == 'd' || mode == 'e' || mode == 's' || mode == 't' || mode == 'u' || mode == 'f') {
    lexer->advance(lexer, false);
  }
  if (lexer->lookahead != 'x') return false;
  uint32_t column = query_column(lexer, mode);
  lexer->advance(lexer, false);
  lexer->mark_end(lexer);
  while (is_gap(lexer->lookahead)) lexer->advance(lexer, false);
  query_column(lexer, mode);
  lexer->result_symbol = column % 2;
  return true;
}

static uint32_t query_column(TSLexer *lexer, int32_t mode) {
  switch (mode) {
    case 'd': return lexer->get_display_column(lexer, 4);
    case 'e': return lexer->get_display_column(lexer, 8);
    case 's': lexer->get_display_column(lexer, 4); return lexer->get_column(lexer);
    case 't': lexer->get_column(lexer); return lexer->get_display_column(lexer, 4);
    case 'u':
      lexer->get_display_column(lexer, 4);
      lexer->get_display_column(lexer, 8);
      return lexer->get_display_column(lexer, 4);
    case 'f':
      lexer->get_display_column(lexer, 8);
      lexer->get_column(lexer);
      return lexer->get_display_column(lexer, 4);
    default: return lexer->get_column(lexer);
  }
}
