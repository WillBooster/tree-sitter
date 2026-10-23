#include "tree_sitter/parser.h"
#include <stdlib.h>

void *tree_sitter_column_display_replay_external_scanner_create(void) { return calloc(1, 1); }
void tree_sitter_column_display_replay_external_scanner_destroy(void *payload) { free(payload); }
unsigned tree_sitter_column_display_replay_external_scanner_serialize(void *payload, char *buffer) {
  buffer[0] = *(char *)payload;
  return 1;
}
void tree_sitter_column_display_replay_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  *(char *)payload = length ? buffer[0] : 0;
}

static void advance_characters(TSLexer *lexer, unsigned count);

bool tree_sitter_column_display_replay_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid) {
  char *stage = payload;
  if (*stage > 1) return false;
  uint32_t column = 0;
  if (*stage == 0) {
    advance_characters(lexer, 1);
    lexer->mark_end(lexer);
    advance_characters(lexer, 7);
    lexer->get_display_column(lexer, 8);
    lexer->get_column(lexer);
  } else {
    advance_characters(lexer, 8);
    lexer->get_display_column(lexer, 4);
    lexer->get_column(lexer);
    while (!lexer->is_at_included_range_start(lexer) && !lexer->eof(lexer)) {
      lexer->advance(lexer, false);
    }
    advance_characters(lexer, 1);
    column = lexer->get_display_column(lexer, 4);
    advance_characters(lexer, 1);
    lexer->mark_end(lexer);
  }
  lexer->result_symbol = column % 2;
  (*stage)++;
  return true;
}

static void advance_characters(TSLexer *lexer, unsigned count) {
  while (count-- && !lexer->eof(lexer)) lexer->advance(lexer, false);
}
