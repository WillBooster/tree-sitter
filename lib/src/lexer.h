#ifndef TREE_SITTER_LEXER_H_
#define TREE_SITTER_LEXER_H_

#ifdef __cplusplus
extern "C" {
#endif

#include "./length.h"
#include "./subtree.h"
#include "tree_sitter/api.h"
#include "./parser.h"

typedef struct {
  uint32_t value;
  bool valid;
} ColumnData;

typedef struct {
  Length position;
  ColumnData column;
  ColumnData display_column;
  uint32_t tab_width;
} ColumnCheckpoint;

typedef struct {
  TSLexer data;
  Length current_position;
  Length token_start_position;
  Length marked_start_position;
  Length token_end_position;

  TSRange *included_ranges;
  const char *chunk;
  TSInput input;
  TSLogger logger;

  uint32_t included_range_count;
  uint32_t current_included_range_index;
  uint32_t chunk_start;
  uint32_t chunk_size;
  uint32_t lookahead_size;
  char lookahead_buffer[4];
  bool did_get_column;
  bool did_get_position;
  ColumnData column_data;
  ColumnData display_column_data;
  uint32_t display_tab_width;
  ColumnCheckpoint start_column_checkpoint;
  ColumnCheckpoint end_column_checkpoint;

  char debug_buffer[TREE_SITTER_SERIALIZATION_BUFFER_SIZE];
} Lexer;

void ts_lexer_init(Lexer *self);
void ts_lexer_delete(Lexer *self);
void ts_lexer_set_input(Lexer *self, TSInput input);
void ts_lexer_reset(Lexer *self, Length position);
void ts_lexer_start(Lexer *self);
void ts_lexer_finish(Lexer *self, uint32_t *lookahead_end_byte);
void ts_lexer_mark_end(Lexer *self);
bool ts_lexer_set_included_ranges(Lexer *self, const TSRange *ranges, uint32_t count);
TSRange *ts_lexer_included_ranges(const Lexer *self, uint32_t *count);

#ifdef __cplusplus
}
#endif

#endif  // TREE_SITTER_LEXER_H_
