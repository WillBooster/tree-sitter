#include "tree_sitter/parser.h"
#include <stdlib.h>

void *tree_sitter_required_extra_eof_recovery_external_scanner_create(void) {
  return calloc(1, sizeof(unsigned char));
}

void tree_sitter_required_extra_eof_recovery_external_scanner_destroy(void *payload) {
  free(payload);
}

unsigned tree_sitter_required_extra_eof_recovery_external_scanner_serialize(void *payload, char *buffer) {
  buffer[0] = *(unsigned char *)payload;
  return 1;
}

void tree_sitter_required_extra_eof_recovery_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  *(unsigned char *)payload = length ? (unsigned char)buffer[0] : 0;
}

bool tree_sitter_required_extra_eof_recovery_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  unsigned char *phase = payload;
  if (!lexer->eof(lexer) || *phase > 1 || !valid_symbols[*phase]) return false;
  lexer->result_symbol = *phase;
  (*phase)++;
  return true;
}
