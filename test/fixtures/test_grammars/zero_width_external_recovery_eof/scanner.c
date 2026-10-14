#include "tree_sitter/parser.h"

#include <stdlib.h>
#include <wctype.h>

enum { CLOSE, END };

void *tree_sitter_zero_width_external_recovery_eof_external_scanner_create(void) {
  return calloc(1, sizeof(char));
}

void tree_sitter_zero_width_external_recovery_eof_external_scanner_destroy(void *payload) {
  free(payload);
}

unsigned tree_sitter_zero_width_external_recovery_eof_external_scanner_serialize(void *payload, char *buffer) {
  buffer[0] = *(char *)payload;
  return 1;
}

void tree_sitter_zero_width_external_recovery_eof_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  *(char *)payload = length ? buffer[0] : 0;
}

bool tree_sitter_zero_width_external_recovery_eof_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  char *phase = payload;
  lexer->mark_end(lexer);
  while (iswspace(lexer->lookahead)) lexer->advance(lexer, false);
  if (!*phase && valid_symbols[END] && lexer->eof(lexer)) {
    lexer->result_symbol = END;
    *phase = 1;
    return true;
  }
  if (valid_symbols[CLOSE] && lexer->lookahead == ')') {
    lexer->advance(lexer, false);
    lexer->mark_end(lexer);
    lexer->result_symbol = CLOSE;
    return true;
  }
  return false;
}
