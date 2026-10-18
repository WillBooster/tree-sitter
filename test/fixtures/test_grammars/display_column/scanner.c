#include "tree_sitter/parser.h"
#include <limits.h>

void *tree_sitter_display_column_external_scanner_create(void) { return NULL; }
void tree_sitter_display_column_external_scanner_destroy(void *payload) {}
unsigned tree_sitter_display_column_external_scanner_serialize(void *payload, char *buffer) { return 0; }
void tree_sitter_display_column_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {}

static bool finish_alignment(TSLexer *lexer, uint32_t column);

bool tree_sitter_display_column_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid) {
  if (!valid[0]) return false;
  if (lexer->lookahead == '^' || lexer->lookahead == '&') {
    if (lexer->lookahead == '^') lexer->get_display_column(lexer, 4);
    lexer->advance(lexer, false);
    while (lexer->lookahead == ' ') lexer->advance(lexer, false);
    return finish_alignment(lexer, lexer->get_column(lexer));
  }
  uint32_t width;
  switch (lexer->lookahead) {
    case '#': width = 8; break;
    case '@': width = 4; break;
    case '$': width = 0; break;
    case '%': width = UINT32_MAX; break;
    default: return false;
  }
  int32_t lookahead = lexer->lookahead;
  TSLexerPosition before, after;
  lexer->get_position(lexer, &before);
  uint32_t scalar_column = lexer->get_column(lexer);
  uint32_t column = lexer->get_display_column(lexer, width);
  lexer->get_display_column(lexer, width == 4 ? 8 : 4);
  if (lexer->get_display_column(lexer, width) != column || lexer->get_column(lexer) != scalar_column) return false;
  lexer->get_position(lexer, &after);
  if (lookahead != lexer->lookahead) return false;
  if (before.bytes != after.bytes || before.row != after.row || before.column != after.column) return false;
  lexer->advance(lexer, false);
  bool matched = finish_alignment(lexer, column);
  if (matched) lexer->get_display_column(lexer, width);
  return matched;
}

static bool finish_alignment(TSLexer *lexer, uint32_t column) {
  uint32_t expected = 0;
  if (lexer->lookahead < '0' || lexer->lookahead > '9') return false;
  while (lexer->lookahead >= '0' && lexer->lookahead <= '9') {
    expected = expected * 10 + lexer->lookahead - '0';
    lexer->advance(lexer, false);
  }
  if (column != expected || lexer->lookahead != '!') return false;
  lexer->mark_end(lexer);
  lexer->advance(lexer, false);
  lexer->result_symbol = 0;
  return true;
}
