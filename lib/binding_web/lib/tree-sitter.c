// code-gauge-allow-file fileNcss 551 -- Wasm C shim keeps shared scratch marshaling buffers in one translation unit
#include "array.h"
#include "point.h"

#include <emscripten.h>
#include <tree_sitter/api.h>
#include "memory_limit.h"

static const uint32_t INPUT_BUFFER_SIZE = 10 * 1024;

const void *TRANSFER_BUFFER[12] = {
  NULL, NULL, NULL, NULL,
  NULL, NULL, NULL, NULL,
  NULL, NULL, NULL, NULL,
};

static const int SIZE_OF_NODE = 5;
static const int SIZE_OF_POINT = 2;

__attribute__((noreturn)) void abort(void) {
  __builtin_trap();
}

void *ts_init() {
  ts_set_allocator(web_malloc, web_calloc, web_realloc, web_free);
  TRANSFER_BUFFER[0] = (const void *)TREE_SITTER_LANGUAGE_VERSION;
  TRANSFER_BUFFER[1] = (const void *)TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION;
  return (void*)TRANSFER_BUFFER;
}

static uint32_t code_unit_to_byte(uint32_t unit) {
  return unit << 1;
}

static uint32_t byte_to_code_unit(uint32_t byte) {
  return byte >> 1;
}

static inline void marshal_node(const void **buffer, TSNode node) {
  buffer[0] = node.id;
  buffer[1] = (const void *)byte_to_code_unit(node.context[0]);
  buffer[2] = (const void *)node.context[1];
  buffer[3] = (const void *)byte_to_code_unit(node.context[2]);
  buffer[4] = (const void *)node.context[3];
}

static inline TSNode unmarshal_node_at(const TSTree *tree, uint32_t index) {
  TSNode node;
  const void **buffer = TRANSFER_BUFFER + index * SIZE_OF_NODE;
  node.id = buffer[0];
  node.context[0] = code_unit_to_byte((uint32_t)buffer[1]);
  node.context[1] = (uint32_t)buffer[2];
  node.context[2] = code_unit_to_byte((uint32_t)buffer[3]);
  node.context[3] = (uint32_t)buffer[4];
  node.tree = tree;
  return node;
}

static inline TSNode unmarshal_node(const TSTree *tree) {
  return unmarshal_node_at(tree, 0);
}

static inline void marshal_cursor_node(const TSTreeCursor *cursor) {
  TSNode node = ts_tree_cursor_current_node(cursor);
  TRANSFER_BUFFER[0] = (const void *)(uintptr_t)ts_node_symbol(node);
  TRANSFER_BUFFER[1] = (const void *)(uintptr_t)ts_tree_cursor_current_field_id(cursor);
  TRANSFER_BUFFER[2] = (const void *)(uintptr_t)byte_to_code_unit(ts_node_start_byte(node));
  TRANSFER_BUFFER[3] = (const void *)(uintptr_t)byte_to_code_unit(ts_node_end_byte(node));
  uint32_t flags = (ts_node_is_named(node) ? 1u : 0u) | (ts_node_is_missing(node) ? 2u : 0u);
  TRANSFER_BUFFER[4] = (const void *)(uintptr_t)flags;
}

static inline uint64_t unmarshal_u64(void) {
  return (uint64_t)(uint32_t)(uintptr_t)TRANSFER_BUFFER[0] |
    ((uint64_t)(uint32_t)(uintptr_t)TRANSFER_BUFFER[1] << 32);
}

static inline void marshal_u64(uint64_t value) {
  TRANSFER_BUFFER[0] = (const void *)(uintptr_t)(uint32_t)value;
  TRANSFER_BUFFER[1] = (const void *)(uintptr_t)(uint32_t)(value >> 32);
}

static void marshal_point(TSPoint point) {
  TRANSFER_BUFFER[0] = (const void *)point.row;
  TRANSFER_BUFFER[1] = (const void *)byte_to_code_unit(point.column);
}

static TSPoint unmarshal_point(const void **address) {
  TSPoint point;
  point.row = (uint32_t)address[0];
  point.column = code_unit_to_byte((uint32_t)address[1]);
  return point;
}

static void marshal_range(TSRange *range) {
  range->start_byte = byte_to_code_unit(range->start_byte);
  range->end_byte = byte_to_code_unit(range->end_byte);
  range->start_point.column = byte_to_code_unit(range->start_point.column);
  range->end_point.column = byte_to_code_unit(range->end_point.column);
}

static void unmarshal_range(TSRange *range) {
  range->start_byte = code_unit_to_byte(range->start_byte);
  range->end_byte = code_unit_to_byte(range->end_byte);
  range->start_point.column = code_unit_to_byte(range->start_point.column);
  range->end_point.column = code_unit_to_byte(range->end_point.column);
}

static TSInputEdit unmarshal_edit() {
  TSInputEdit edit;
  const void **address = TRANSFER_BUFFER;
  edit.start_point = unmarshal_point(address); address += SIZE_OF_POINT;
  edit.old_end_point = unmarshal_point(address); address += SIZE_OF_POINT;
  edit.new_end_point = unmarshal_point(address); address += SIZE_OF_POINT;
  edit.start_byte = code_unit_to_byte((uint32_t)*address); address += 1;
  edit.old_end_byte = code_unit_to_byte((uint32_t)*address); address += 1;
  edit.new_end_byte = code_unit_to_byte((uint32_t)*address); address += 1;
  return edit;
}

static void marshal_language_metadata(const TSLanguageMetadata *metadata) {
  if (metadata == NULL) {
    TRANSFER_BUFFER[0] = 0;
    return;
  }
  TRANSFER_BUFFER[0] = (const void*)3;
  TRANSFER_BUFFER[1] = (const void*)(uint32_t)metadata->major_version;
  TRANSFER_BUFFER[2] = (const void*)(uint32_t)metadata->minor_version;
  TRANSFER_BUFFER[3] = (const void*)(uint32_t)metadata->patch_version;
}

extern void tree_sitter_parse_callback(
  char *input_buffer,
  uint32_t max_code_units,
  uint32_t index,
  uint32_t row,
  uint32_t column,
  uint32_t *length_read
);

extern void tree_sitter_log_callback(
  bool is_lex_message,
  const char *message
);

extern bool tree_sitter_progress_callback(
  uint32_t current_offset,
  bool has_error
);

extern void ts_parser_abandon_after_alloc_failure(TSParser *parser);
extern void ts_parser_set_max_operations(TSParser *parser, uint64_t max_operations);
extern uint64_t ts_parser_last_parse_operations(const TSParser *parser);

void ts_free_wasm(void *pointer) {
  ts_free(pointer);
}

void *ts_scanner_malloc_wasm(size_t size) {
  void *result = malloc(size);
  if (!result && size && current_parse_budget) web_allocation_failed();
  return result;
}

void *ts_scanner_calloc_wasm(size_t count, size_t size) {
  void *result = calloc(count, size);
  if (!result && count && size && current_parse_budget) web_allocation_failed();
  return result;
}

void *ts_scanner_realloc_wasm(void *pointer, size_t size) {
  void *result = realloc(pointer, size);
  if (!result && size && current_parse_budget) web_allocation_failed();
  return result;
}

static const char *call_parse_callback(
  void *payload,
  uint32_t byte,
  TSPoint position,
  uint32_t *bytes_read
) {
  char *buffer = (char *)payload;
  tree_sitter_parse_callback(
    buffer,
    byte_to_code_unit(INPUT_BUFFER_SIZE),
    byte_to_code_unit(byte),
    position.row,
    byte_to_code_unit(position.column),
    bytes_read
  );
  *bytes_read = code_unit_to_byte(*bytes_read);
  if (*bytes_read > INPUT_BUFFER_SIZE) {
    *bytes_read = INPUT_BUFFER_SIZE;
  }
  return buffer;
}

static void call_log_callback(
  void *payload,
  TSLogType log_type,
  const char *message
) {
  tree_sitter_log_callback(log_type == TSLogTypeLex, message);
}

static bool progress_callback(
  TSParseState *state
) {
  return tree_sitter_progress_callback(state->current_byte_offset, state->has_error);
}

void ts_parser_new_wasm() {
  TSParser *parser = ts_parser_new();
  char *input_buffer = calloc(INPUT_BUFFER_SIZE, sizeof(char));
  TRANSFER_BUFFER[0] = parser;
  TRANSFER_BUFFER[1] = input_buffer;
}

void ts_parser_enable_logger_wasm(TSParser *self, bool should_log) {
  TSLogger logger = {self, should_log ? call_log_callback : NULL};
  ts_parser_set_logger(self, logger);
}

typedef struct {
  TSRange *ranges;
  uint32_t range_count;
  uint64_t max_operations;
  bool has_progress_callback;
} WasmParseInputs;

static bool has_default_included_range(const TSParser *self) {
  uint32_t count = 0;
  const TSRange *ranges = ts_parser_included_ranges(self, &count);
  return count == 1 &&
    ranges[0].start_byte == 0 &&
    ranges[0].end_byte == UINT32_MAX &&
    ranges[0].start_point.row == 0 &&
    ranges[0].start_point.column == 0 &&
    ranges[0].end_point.row == UINT32_MAX &&
    ranges[0].end_point.column == UINT32_MAX;
}

static void apply_included_ranges(
  TSParser *self,
  TSRange **ranges_ptr,
  uint32_t range_count
) {
  TSRange *ranges = *ranges_ptr;
  if (range_count) {
    for (uint32_t i = 0; i < range_count; i++) {
      unmarshal_range(&ranges[i]);
    }
    ts_parser_set_included_ranges(self, ranges, range_count);
  } else if (!has_default_included_range(self)) {
    ts_parser_set_included_ranges(self, NULL, 0);
  }
  *ranges_ptr = NULL;
  free(ranges);
}

static void unmarshal_parse_inputs(WasmParseInputs *out) {
  out->max_operations = unmarshal_u64();
  out->ranges = (TSRange *)TRANSFER_BUFFER[2];
  out->range_count = (uint32_t)(uintptr_t)TRANSFER_BUFFER[3];
  out->has_progress_callback = (uintptr_t)TRANSFER_BUFFER[4] != 0;
}

static TSTree *parse_with_callback(
  TSParser *self,
  char *input_buffer,
  const TSTree *old_tree,
  bool has_progress_callback
) {
  TSInput input = {input_buffer, call_parse_callback, TSInputEncodingUTF16LE, NULL};
  TSParseOptions options = {
    .payload = NULL,
    .progress_callback = has_progress_callback ? progress_callback : NULL,
  };
  return ts_parser_parse_with_options(self, old_tree, input, options);
}

TSTree *ts_parser_parse_wasm(
  TSParser *self,
  char *input_buffer,
  const TSTree *old_tree
) {
  WasmParseInputs parsed;
  unmarshal_parse_inputs(&parsed);
  apply_included_ranges(self, &parsed.ranges, parsed.range_count);
  ts_parser_set_max_operations(self, parsed.max_operations);
  TSTree *tree = parse_with_callback(self, input_buffer, old_tree, parsed.has_progress_callback);
  marshal_u64(ts_parser_last_parse_operations(self));
  return tree;
}

static TSParser *create_limited_parser(
  const TSParser *self,
  WebParseBudget *budget,
  const WasmParseInputs *parsed
) {
  // An allocation failure can leave a stack half-updated, so the caller's parser is not used for a limited parse.
  TSParser *limited_parser = ts_parser_new();
  budget->parser = limited_parser;
  ts_parser_set_language(limited_parser, ts_parser_language(self));
  ts_parser_set_logger(limited_parser, ts_parser_logger(self));
  apply_included_ranges(limited_parser, (TSRange **)&budget->ranges, parsed->range_count);
  ts_parser_set_max_operations(limited_parser, parsed->max_operations);
  return limited_parser;
}

TSTree *ts_parser_parse_limited_wasm(
  TSParser *self,
  char *input_buffer
) {
  WasmParseInputs parsed;
  unmarshal_parse_inputs(&parsed);
  WebParseBudget *budget = web_budget_new(parsed.ranges);
  if (!budget) {
    marshal_u64(0);
    return NULL;
  }
  if (setjmp(budget->jump)) {
    current_parse_budget = NULL;
    uint64_t operations = budget->parser ? ts_parser_last_parse_operations(budget->parser) : 0;
    if (budget->parser) ts_parser_abandon_after_alloc_failure(budget->parser);
    web_budget_discard(budget);
    marshal_u64(operations);
    return NULL;
  }

  current_parse_budget = budget;
  TSParser *limited_parser = create_limited_parser(self, budget, &parsed);
  TSTree *tree = parse_with_callback(limited_parser, input_buffer, NULL, parsed.has_progress_callback);
  uint64_t operations = ts_parser_last_parse_operations(limited_parser);
  if (parsed.range_count || !has_default_included_range(self)) {
    uint32_t included_range_count;
    const TSRange *included_ranges = ts_parser_included_ranges(limited_parser, &included_range_count);
    ts_parser_set_included_ranges(self, included_ranges, included_range_count);
  }
  ts_parser_delete(limited_parser);
  budget->parser = NULL;
  web_budget_finish(budget);
  marshal_u64(operations);
  return tree;
}

void ts_parser_last_parse_operations_wasm(const TSParser *self) {
  marshal_u64(ts_parser_last_parse_operations(self));
}

void ts_parser_abort_limited_wasm(void) {
  uint64_t operations = 0;
  if (current_parse_budget) {
    WebParseBudget *budget = current_parse_budget;
    current_parse_budget = NULL;
    if (budget->parser) {
      operations = ts_parser_last_parse_operations(budget->parser);
      ts_parser_abandon_after_alloc_failure(budget->parser);
    }
    web_budget_discard(budget);
  }
  marshal_u64(operations);
}

void ts_parser_included_ranges_wasm(TSParser *self) {
  uint32_t range_count = 0;
  const TSRange *ranges = ts_parser_included_ranges(self, &range_count);
  TSRange *copied_ranges = malloc(sizeof(TSRange) * range_count);
  memcpy(copied_ranges, ranges, sizeof(TSRange) * range_count);
  for (unsigned i = 0; i < range_count; i++) {
    marshal_range(&copied_ranges[i]);
  }
  TRANSFER_BUFFER[0] = range_count ? (const void *)range_count : NULL;
  TRANSFER_BUFFER[1] = copied_ranges;
}

int ts_language_type_is_named_wasm(const TSLanguage *self, TSSymbol typeId) {
  const TSSymbolType symbolType = ts_language_symbol_type(self, typeId);
  return symbolType == TSSymbolTypeRegular;
}

int ts_language_type_is_visible_wasm(const TSLanguage *self, TSSymbol typeId) {
  const TSSymbolType symbolType = ts_language_symbol_type(self, typeId);
  return symbolType <= TSSymbolTypeAnonymous;
}

void ts_language_metadata_wasm(const TSLanguage *self) {
  const TSLanguageMetadata *metadata = ts_language_metadata(self);
  marshal_language_metadata(metadata);
}

void ts_language_supertypes_wasm(const TSLanguage *self) {
  uint32_t length;
  const TSSymbol *supertypes = ts_language_supertypes(self, &length);
  TRANSFER_BUFFER[0] = (const void *)length;
  TRANSFER_BUFFER[1] = supertypes;
}

void ts_language_subtypes_wasm(const TSLanguage *self, TSSymbol supertype) {
  uint32_t length;
  const TSSymbol *subtypes = ts_language_subtypes(self, supertype, &length);
  TRANSFER_BUFFER[0] = (const void *)length;
  TRANSFER_BUFFER[1] = subtypes;
}

void ts_tree_root_node_wasm(const TSTree *tree) {
  marshal_node(TRANSFER_BUFFER, ts_tree_root_node(tree));
}

void ts_tree_root_node_with_offset_wasm(const TSTree *tree) {
  const void **address = TRANSFER_BUFFER + SIZE_OF_NODE;
  uint32_t offset = code_unit_to_byte((uint32_t)address[0]);
  TSPoint extent = unmarshal_point(address + 1);
  TSNode node = ts_tree_root_node_with_offset(tree, offset, extent);
  marshal_node(TRANSFER_BUFFER, node);
}

void ts_tree_edit_wasm(TSTree *tree) {
  TSInputEdit edit = unmarshal_edit();
  ts_tree_edit(tree, &edit);
}

void ts_tree_included_ranges_wasm(const TSTree *tree) {
  uint32_t range_count;
  TSRange *ranges = ts_tree_included_ranges(tree, &range_count);
  for (unsigned i = 0; i < range_count; i++) {
    marshal_range(&ranges[i]);
  }
  TRANSFER_BUFFER[0] = (range_count ? (const void *)range_count : NULL);
  TRANSFER_BUFFER[1] = (const void *)ranges;
}

void ts_tree_get_changed_ranges_wasm(TSTree *tree, TSTree *other) {
  unsigned range_count;
  TSRange *ranges = ts_tree_get_changed_ranges(tree, other, &range_count);
  for (unsigned i = 0; i < range_count; i++) {
    marshal_range(&ranges[i]);
  }
  TRANSFER_BUFFER[0] = (const void *)range_count;
  TRANSFER_BUFFER[1] = (const void *)ranges;
}

TSTreeCursor *ts_tree_cursor_new_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  TSTreeCursor *cursor = ts_malloc(sizeof(TSTreeCursor));
  *cursor = ts_tree_cursor_new(node);
  marshal_cursor_node(cursor);
  return cursor;
}

TSTreeCursor *ts_tree_cursor_copy_wasm(const TSTreeCursor *cursor) {
  TSTreeCursor *copy = ts_malloc(sizeof(TSTreeCursor));
  *copy = ts_tree_cursor_copy(cursor);
  marshal_cursor_node(copy);
  return copy;
}

void ts_tree_cursor_delete_wasm(TSTreeCursor *cursor) {
  if (!cursor) return;
  ts_tree_cursor_delete(cursor);
  ts_free(cursor);
}

void ts_tree_cursor_reset_wasm(TSTreeCursor *cursor, const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  ts_tree_cursor_reset(cursor, node);
  marshal_cursor_node(cursor);
}

void ts_tree_cursor_reset_to_wasm(TSTreeCursor *dst, const TSTreeCursor *src) {
  ts_tree_cursor_reset_to(dst, src);
  marshal_cursor_node(dst);
}

bool ts_tree_cursor_goto_first_child_wasm(TSTreeCursor *cursor) {
  bool result = ts_tree_cursor_goto_first_child(cursor);
  if (result) marshal_cursor_node(cursor);
  return result;
}

bool ts_tree_cursor_goto_last_child_wasm(TSTreeCursor *cursor) {
  bool result = ts_tree_cursor_goto_last_child(cursor);
  if (result) marshal_cursor_node(cursor);
  return result;
}

bool ts_tree_cursor_goto_first_child_for_index_wasm(TSTreeCursor *cursor, uint32_t index) {
  bool result = ts_tree_cursor_goto_first_child_for_byte(cursor, code_unit_to_byte(index)) >= 0;
  if (result) marshal_cursor_node(cursor);
  return result;
}

bool ts_tree_cursor_goto_first_child_for_position_wasm(
  TSTreeCursor *cursor,
  uint32_t row,
  uint32_t column
) {
  TSPoint point = {row, code_unit_to_byte(column)};
  bool result = ts_tree_cursor_goto_first_child_for_point(cursor, point) >= 0;
  if (result) marshal_cursor_node(cursor);
  return result;
}

bool ts_tree_cursor_goto_next_sibling_wasm(TSTreeCursor *cursor) {
  bool result = ts_tree_cursor_goto_next_sibling(cursor);
  if (result) marshal_cursor_node(cursor);
  return result;
}

bool ts_tree_cursor_goto_previous_sibling_wasm(TSTreeCursor *cursor) {
  bool result = ts_tree_cursor_goto_previous_sibling(cursor);
  if (result) marshal_cursor_node(cursor);
  return result;
}

void ts_tree_cursor_goto_descendant_wasm(TSTreeCursor *cursor, uint32_t goal_descendant_index) {
  ts_tree_cursor_goto_descendant(cursor, goal_descendant_index);
  marshal_cursor_node(cursor);
}

bool ts_tree_cursor_goto_parent_wasm(TSTreeCursor *cursor) {
  bool result = ts_tree_cursor_goto_parent(cursor);
  if (result) marshal_cursor_node(cursor);
  return result;
}

uint16_t ts_tree_cursor_current_node_state_id_wasm(const TSTreeCursor *cursor) {
  return ts_node_parse_state(ts_tree_cursor_current_node(cursor));
}

uint32_t ts_tree_cursor_current_node_id_wasm(const TSTreeCursor *cursor) {
  return (uint32_t)ts_tree_cursor_current_node(cursor).id;
}

void ts_tree_cursor_start_position_wasm(const TSTreeCursor *cursor) {
  marshal_point(ts_node_start_point(ts_tree_cursor_current_node(cursor)));
}

void ts_tree_cursor_end_position_wasm(const TSTreeCursor *cursor) {
  marshal_point(ts_node_end_point(ts_tree_cursor_current_node(cursor)));
}

uint32_t ts_tree_cursor_start_index_wasm(const TSTreeCursor *cursor) {
  return byte_to_code_unit(ts_node_start_byte(ts_tree_cursor_current_node(cursor)));
}

uint32_t ts_tree_cursor_end_index_wasm(const TSTreeCursor *cursor) {
  return byte_to_code_unit(ts_node_end_byte(ts_tree_cursor_current_node(cursor)));
}

void ts_tree_cursor_current_node_wasm(const TSTreeCursor *cursor) {
  marshal_node(TRANSFER_BUFFER, ts_tree_cursor_current_node(cursor));
}

static TSTreeCursor scratch_cursor = {0};

uint16_t ts_node_symbol_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_symbol(node);
}

const char *ts_node_field_name_for_child_wasm(const TSTree *tree, uint32_t index) {
  TSNode node = unmarshal_node(tree);
  return ts_node_field_name_for_child(node, index);
}

const char *ts_node_field_name_for_named_child_wasm(const TSTree *tree, uint32_t index) {
  TSNode node = unmarshal_node(tree);
  return ts_node_field_name_for_named_child(node, index);
}

void ts_node_children_by_field_id_wasm(const TSTree *tree, uint32_t field_id) {
  TSNode node = unmarshal_node(tree);
  TSTreeCursor cursor = ts_tree_cursor_new(node);

  bool done = field_id == 0;
  if (!done) {
    ts_tree_cursor_reset(&cursor, node);
    ts_tree_cursor_goto_first_child(&cursor);
  }

  Array(const void*) result = array_new();

  while (!done) {
    while (ts_tree_cursor_current_field_id(&cursor) != field_id) {
      if (!ts_tree_cursor_goto_next_sibling(&cursor)) {
        done = true;
        break;
      }
    }
    if (done) {
      break;
    }
    TSNode result_node = ts_tree_cursor_current_node(&cursor);
    if (!ts_tree_cursor_goto_next_sibling(&cursor)) {
      done = true;
    }
    array_grow_by(&result, SIZE_OF_NODE);
    marshal_node(result.contents + result.size - SIZE_OF_NODE, result_node);
  }
  ts_tree_cursor_delete(&cursor);

  TRANSFER_BUFFER[0] = (const void*)(result.size / SIZE_OF_NODE);
  TRANSFER_BUFFER[1] = (const void*)result.contents;
}

void ts_node_first_child_for_byte_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  const void** address = TRANSFER_BUFFER + SIZE_OF_NODE;
  uint32_t byte = code_unit_to_byte((uint32_t)address[0]);
  marshal_node(TRANSFER_BUFFER, ts_node_first_child_for_byte(node, byte));
}

void ts_node_first_named_child_for_byte_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  const void** address = TRANSFER_BUFFER + SIZE_OF_NODE;
  uint32_t byte = code_unit_to_byte((uint32_t)address[0]);
  marshal_node(TRANSFER_BUFFER, ts_node_first_named_child_for_byte(node, byte));
}

uint16_t ts_node_grammar_symbol_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_grammar_symbol(node);
}

uint32_t ts_node_child_count_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_child_count(node);
}

uint32_t ts_node_named_child_count_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_named_child_count(node);
}

void ts_node_child_wasm(const TSTree *tree, uint32_t index) {
  TSNode node = unmarshal_node(tree);
  marshal_node(TRANSFER_BUFFER, ts_node_child(node, index));
}

void ts_node_named_child_wasm(const TSTree *tree, uint32_t index) {
  TSNode node = unmarshal_node(tree);
  marshal_node(TRANSFER_BUFFER, ts_node_named_child(node, index));
}

void ts_node_child_by_field_id_wasm(const TSTree *tree, uint32_t field_id) {
  TSNode node = unmarshal_node(tree);
  marshal_node(TRANSFER_BUFFER, ts_node_child_by_field_id(node, field_id));
}

void ts_node_next_sibling_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  marshal_node(TRANSFER_BUFFER, ts_node_next_sibling(node));
}

void ts_node_prev_sibling_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  marshal_node(TRANSFER_BUFFER, ts_node_prev_sibling(node));
}

void ts_node_next_named_sibling_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  marshal_node(TRANSFER_BUFFER, ts_node_next_named_sibling(node));
}

void ts_node_prev_named_sibling_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  marshal_node(TRANSFER_BUFFER, ts_node_prev_named_sibling(node));
}

uint32_t ts_node_descendant_count_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_descendant_count(node);
}

void ts_node_parent_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  marshal_node(TRANSFER_BUFFER, ts_node_parent(node));
}

void ts_node_child_with_descendant_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  TSNode descendant = unmarshal_node_at(tree, 1);
  marshal_node(TRANSFER_BUFFER, ts_node_child_with_descendant(node, descendant));
}

void ts_node_descendant_for_index_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  const void **address = TRANSFER_BUFFER + SIZE_OF_NODE;
  uint32_t start = code_unit_to_byte((uint32_t)address[0]);
  uint32_t end = code_unit_to_byte((uint32_t)address[1]);
  marshal_node(TRANSFER_BUFFER, ts_node_descendant_for_byte_range(node, start, end));
}

void ts_node_named_descendant_for_index_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  const void **address = TRANSFER_BUFFER + SIZE_OF_NODE;
  uint32_t start = code_unit_to_byte((uint32_t)address[0]);
  uint32_t end = code_unit_to_byte((uint32_t)address[1]);
  marshal_node(TRANSFER_BUFFER, ts_node_named_descendant_for_byte_range(node, start, end));
}

void ts_node_descendant_for_position_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  const void **address = TRANSFER_BUFFER + SIZE_OF_NODE;
  TSPoint start = unmarshal_point(address); address += SIZE_OF_POINT;
  TSPoint end = unmarshal_point(address);
  marshal_node(TRANSFER_BUFFER, ts_node_descendant_for_point_range(node, start, end));
}

void ts_node_named_descendant_for_position_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  const void **address = TRANSFER_BUFFER + SIZE_OF_NODE;
  TSPoint start = unmarshal_point(address); address += SIZE_OF_POINT;
  TSPoint end = unmarshal_point(address);
  marshal_node(TRANSFER_BUFFER, ts_node_named_descendant_for_point_range(node, start, end));
}

void ts_node_end_point_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  marshal_point(ts_node_end_point(node));
}

uint32_t ts_node_end_index_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return byte_to_code_unit(ts_node_end_byte(node));
}

char *ts_node_to_string_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_string(node);
}

static void marshal_node_children(const TSTree *tree, bool named_only) {
  TSNode node = unmarshal_node(tree);
  uint32_t count = named_only ? ts_node_named_child_count(node) : ts_node_child_count(node);
  const void **result = NULL;
  if (count > 0) {
    result = (const void **)calloc(sizeof(void *), SIZE_OF_NODE * count);
    const void **address = result;
    ts_tree_cursor_reset(&scratch_cursor, node);
    ts_tree_cursor_goto_first_child(&scratch_cursor);
    uint32_t i = 0;
    do {
      TSNode child = ts_tree_cursor_current_node(&scratch_cursor);
      if (!named_only || ts_node_is_named(child)) {
        marshal_node(address, child);
        address += SIZE_OF_NODE;
        if (++i == count) break;
      }
    } while (ts_tree_cursor_goto_next_sibling(&scratch_cursor));
  }
  TRANSFER_BUFFER[0] = (const void *)count;
  TRANSFER_BUFFER[1] = (const void *)result;
}

void ts_node_children_wasm(const TSTree *tree) {
  marshal_node_children(tree, false);
}

void ts_node_named_children_wasm(const TSTree *tree) {
  marshal_node_children(tree, true);
}

bool symbols_contain(const uint32_t *set, uint32_t length, uint32_t value) {
  for (unsigned i = 0; i < length; i++) {
    if (set[i] == value) {
      return true;
    }
    if (set[i] > value) {
      break;
    }
  }
  return false;
}

static bool advance_to_next_branch(TSTreeCursor *cursor) {
  while (!ts_tree_cursor_goto_next_sibling(cursor)) {
    if (!ts_tree_cursor_goto_parent(cursor)) return false;
  }
  return true;
}

void ts_node_descendants_of_type_wasm(
  const TSTree *tree,
  const uint32_t *symbols,
  uint32_t symbol_count
) {
  TSNode node = unmarshal_node(tree);
  const void **address = TRANSFER_BUFFER + SIZE_OF_NODE;
  TSPoint start_point = unmarshal_point(address);
  TSPoint end_point = unmarshal_point(address + SIZE_OF_POINT);
  if (end_point.row == 0 && end_point.column == 0) {
    end_point = (TSPoint) {UINT32_MAX, UINT32_MAX};
  }

  Array(const void *) result = array_new();
  ts_tree_cursor_reset(&scratch_cursor, node);
  for (;;) {
    TSNode descendant = ts_tree_cursor_current_node(&scratch_cursor);
    if (point_lte(ts_node_end_point(descendant), start_point)) {
      if (!advance_to_next_branch(&scratch_cursor)) break;
      continue;
    }
    if (point_lte(end_point, ts_node_start_point(descendant))) break;
    if (symbols_contain(symbols, symbol_count, ts_node_symbol(descendant))) {
      array_grow_by(&result, SIZE_OF_NODE);
      marshal_node(result.contents + result.size - SIZE_OF_NODE, descendant);
    }
    if (!ts_tree_cursor_goto_first_child(&scratch_cursor) &&
        !advance_to_next_branch(&scratch_cursor)) {
      break;
    }
  }

  TRANSFER_BUFFER[0] = (const void *)(result.size / SIZE_OF_NODE);
  TRANSFER_BUFFER[1] = (const void *)result.contents;
}

int ts_node_is_named_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_is_named(node);
}

int ts_node_has_changes_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_has_changes(node);
}

int ts_node_has_error_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_has_error(node);
}

int ts_node_is_error_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_is_error(node);
}

int ts_node_is_missing_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_is_missing(node);
}

int ts_node_is_extra_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_is_extra(node);
}

uint16_t ts_node_parse_state_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_parse_state(node);
}

uint16_t ts_node_next_parse_state_wasm(const TSTree *tree) {
  TSNode node = unmarshal_node(tree);
  return ts_node_next_parse_state(node);
}
