#include "tree_sitter/parser.h"

#include <stdlib.h>
#include <wctype.h>

enum { CONCAT, OPEN, CLOSE };

void *tree_sitter_keyword_reuse_external_scanner_create(void) {
  return calloc(1, sizeof(char));
}

void tree_sitter_keyword_reuse_external_scanner_destroy(void *payload) {
  free(payload);
}

unsigned tree_sitter_keyword_reuse_external_scanner_serialize(void *payload, char *buffer) {
  buffer[0] = *(char *)payload;
  return 1;
}

void tree_sitter_keyword_reuse_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  *(char *)payload = length ? buffer[0] : 0;
}

bool tree_sitter_keyword_reuse_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  char *open = payload;
  if (lexer->lookahead == '`' && valid_symbols[*open ? CLOSE : OPEN]) {
    lexer->result_symbol = *open ? CLOSE : OPEN;
    *open = !*open;
    lexer->advance(lexer, false);
    lexer->mark_end(lexer);
    return true;
  }
  if (valid_symbols[CONCAT] && lexer->lookahead && !iswspace(lexer->lookahead) &&
      lexer->lookahead != ';' && lexer->lookahead != '&' && lexer->lookahead != '`') {
    lexer->result_symbol = CONCAT;
    lexer->mark_end(lexer);
    return true;
  }
  return false;
}
