#ifndef TREE_SITTER_LEXER_H_
#define TREE_SITTER_LEXER_H_

#ifdef __cplusplus
extern "C" {
#endif

#include "./length.h"
#include "./subtree.h"
#include "tree_sitter/api.h"
#include "./parser.h"

extern const TSRange TS_DEFAULT_RANGE;

typedef struct {
  uint32_t value;
  bool valid;
  bool valid_for_replay;
} ColumnData;

typedef struct {
  Length position;
  ColumnData column;
  ColumnData display_column;
  uint32_t tab_width;
} ColumnCheckpoint;

typedef struct {
  const uint16_t *table;
  uint32_t total_words;
  uint16_t state_count;
  uint16_t template_count;
  uint16_t class_count;
  uint16_t unicode_range_count;
  uint16_t tag_count;
  bool wide_base;
  bool direct_keyword_lookup;
  uint32_t comb_len;
  uint32_t ext_transitions_len;
  uint32_t ext_accepts_len;
  const uint16_t *ascii_class;
  const uint16_t *unicode_ends;
  const uint16_t *unicode_classes;
  const uint16_t *state_meta;
  const uint16_t *comb;
  const uint16_t *ext_transitions;
  const uint16_t *ext_accepts;
} TSLexTableView;

typedef struct {
  TSLexer data;
  Length current_position;
  Length token_start_position;
  Length marked_start_position;
  Length token_end_position;

  const TSRange *included_ranges;
  const char *chunk;
  TSInput input;
  TSLogger logger;

  uint32_t included_range_count;
  uint32_t current_included_range_index;
  uint32_t chunk_start;
  uint32_t chunk_size;
  uint32_t lookahead_size;
  uint32_t ascii_stride;
  uint32_t ascii_limit;
  uint32_t advance_count;
  bool (*check_progress)(void *payload, unsigned operations);
  void *progress_payload;
  char lookahead_buffer[4];
  ColumnData column_data;
  ColumnData display_column_data;
  uint32_t display_tab_width;
  ColumnCheckpoint start_column_checkpoint;
  ColumnCheckpoint end_column_checkpoint;
  ColumnCheckpoint query_column_checkpoint;
  ColumnCheckpoint query_display_column_checkpoint;
  TSLexTableView main_lex_cache;
  TSLexTableView kw_lex_cache;
  bool track_columns;
  bool has_operation_limit;

  char debug_buffer[TREE_SITTER_SERIALIZATION_BUFFER_SIZE];
} Lexer;

void ts_lexer_init(Lexer *self);
void ts_lexer_delete(Lexer *self);
void ts_lexer_set_input(Lexer *self, TSInput input);
void ts_lexer_set_logger(Lexer *self, TSLogger logger);
void ts_lexer_set_progress_callback(
  Lexer *self,
  bool (*check_progress)(void *payload, unsigned operations),
  void *payload
);
void ts_lexer_reset(Lexer *self, Length position);
void ts_lexer_start(Lexer *self);
void ts_lexer_finish(Lexer *self);
void ts_lexer_get_lookahead(Lexer *self);
bool ts_lexer_flush_operations(Lexer *self);
void ts_lexer_mark_end(Lexer *self);
bool ts_lexer_set_included_ranges(Lexer *self, const TSRange *ranges, uint32_t count);
const TSRange *ts_lexer_included_ranges(const Lexer *self, uint32_t *count);

#ifdef __cplusplus
}
#endif

#endif  // TREE_SITTER_LEXER_H_
