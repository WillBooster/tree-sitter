#include "tree_sitter/parser.h"

#include <stddef.h>

enum { CAST_OPERAND_START, CAST_HEAD_END };

static bool starts_operand(int32_t character);

void *tree_sitter_merged_stack_dynamic_precedence_external_scanner_create(void) {
  return NULL;
}

void tree_sitter_merged_stack_dynamic_precedence_external_scanner_destroy(void *payload) {
  (void)payload;
}

unsigned tree_sitter_merged_stack_dynamic_precedence_external_scanner_serialize(void *payload, char *buffer) {
  (void)payload;
  // An explicit empty state keeps external-token histories involved in stack merging.
  buffer[0] = 0;
  return 1;
}

void tree_sitter_merged_stack_dynamic_precedence_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}

bool tree_sitter_merged_stack_dynamic_precedence_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  (void)payload;
  if (valid_symbols[CAST_OPERAND_START] && valid_symbols[CAST_HEAD_END]) return false;
  while (lexer->lookahead == ' ' || lexer->lookahead == '\n') lexer->advance(lexer, true);
  if (valid_symbols[CAST_OPERAND_START]) {
    if (!starts_operand(lexer->lookahead)) return false;
    lexer->mark_end(lexer);
    lexer->result_symbol = CAST_OPERAND_START;
    return true;
  }
  if (valid_symbols[CAST_HEAD_END] && lexer->lookahead == ')') {
    lexer->advance(lexer, false);
    lexer->mark_end(lexer);
    while (lexer->lookahead == ' ') lexer->advance(lexer, false);
    if (!starts_operand(lexer->lookahead)) return false;
    lexer->result_symbol = CAST_HEAD_END;
    return true;
  }
  return false;
}

static bool starts_operand(int32_t character) {
  return character == '(' || (character >= 'a' && character <= 'z') || (character >= 'A' && character <= 'Z');
}
