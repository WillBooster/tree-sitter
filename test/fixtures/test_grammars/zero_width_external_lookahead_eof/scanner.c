#include "tree_sitter/parser.h"

#include <stdlib.h>

enum { ZERO_WIDTH, SENTINEL, TOKEN_COUNT };

void *tree_sitter_zero_width_external_lookahead_eof_external_scanner_create(void) {
  return calloc(1, sizeof(char));
}

void tree_sitter_zero_width_external_lookahead_eof_external_scanner_destroy(void *payload) {
  free(payload);
}

unsigned tree_sitter_zero_width_external_lookahead_eof_external_scanner_serialize(void *payload, char *buffer) {
  buffer[0] = *(char *)payload;
  return 1;
}

void tree_sitter_zero_width_external_lookahead_eof_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  *(char *)payload = length ? buffer[0] : 0;
}

bool tree_sitter_zero_width_external_lookahead_eof_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  char *phase = payload;
  if (!valid_symbols[ZERO_WIDTH] || !valid_symbols[SENTINEL] || *phase >= TOKEN_COUNT || lexer->eof(lexer)) return false;

  lexer->mark_end(lexer);
  while (!lexer->eof(lexer)) lexer->advance(lexer, false);
  lexer->result_symbol = (*phase)++;
  return true;
}
