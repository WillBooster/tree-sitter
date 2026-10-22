#ifndef TREE_SITTER_LANGUAGE_H_
#define TREE_SITTER_LANGUAGE_H_

#ifdef __cplusplus
extern "C" {
#endif

#include "./subtree.h"
#include "./parser.h"

#define ts_builtin_sym_error_repeat (ts_builtin_sym_error - 1)

#define SMALL_STATE_BITMAP_FLAG 0x40000000u
#define SMALL_STATE_PAIR_FLAG 0x80000000u
#define SMALL_STATE_SET_FLAG (SMALL_STATE_BITMAP_FLAG | SMALL_STATE_PAIR_FLAG)

// Each small state has two `small_parse_table_map` entries: its terminal row, then its goto row. A goto
// entry with `SMALL_STATE_DEFAULTS_FLAG` instead locates a descriptor of two 32-bit words: the offset of the
// set of nonterminals that have gotos, and the map entry (offset and encoding flags) of the row of gotos that
// differ from their nonterminal's default. The defaults start `small_parse_table`, indexed by the
// nonterminal's position after the tokens.
#define SMALL_STATE_DEFAULTS_FLAG 0x20000000u

typedef struct {
  const uint32_t *actions;
  uint32_t action_count;
  bool is_reusable;
} TableEntry;

typedef enum {
  LookaheadFresh,      // no `next()` yet
  LookaheadPositioned, // last `next()` returned true
  LookaheadDone,       // last `next()` returned false
} LookaheadPhase;

typedef struct {
  const TSLanguage *language;
  const uint16_t *data;
  const uint16_t *group_end;
  const uint16_t *set_data;
  uint32_t set_bits;
  uint32_t set_symbol;
  uint16_t table_value;
  uint16_t group_count;
  uint32_t goto_row;
  uint16_t first_symbol;
  uint16_t row_symbol_count;
  bool uses_goto_defaults;
  bool is_small_state;
  bool is_goto_row;
  bool is_pair_state;
  bool is_bitmap_state;
  bool is_set_state;
  TSStateId state;
  LookaheadPhase phase;

  const uint32_t *actions;
  TSSymbol symbol;
  TSStateId next_state;
  uint16_t action_count;
} LookaheadIterator;

// Languages with another ABI version lack `primary_state_ids` and the fields after it or lay them out differently,
// so the accessors that work on a language before a parser or a query has accepted it must not read them.
static inline bool ts_language_has_supported_abi(const TSLanguage *self) {
  return self->abi_version == TREE_SITTER_LANGUAGE_VERSION;
}

static inline uint32_t ts_language__count_ones(uint32_t bits);
static inline uint32_t ts_language__trailing_zeros(uint32_t bits);

uint16_t ts_language_lookup_small_compact(const TSLanguage *self, uint32_t index, TSSymbol symbol);

static inline uint32_t ts_language__small_offset(const uint16_t *words) {
  return words[0] | (uint32_t)words[1] << 16;
}

// Whether the shared symbol set at `offset` in `small_parse_table` contains `symbol`.
static inline bool ts_language_small_set_contains(const TSLanguage *self, uint32_t offset, TSSymbol symbol) {
  const uint16_t *set = &self->small_parse_table[offset];
  uint32_t word = symbol / 16u - set[0];
  return word < set[1] && (set[word + 2] & (1u << (symbol % 16)));
}

// Look up a goto through a descriptor located by a map entry with `SMALL_STATE_DEFAULTS_FLAG`.
static inline uint16_t ts_language_lookup_goto_with_defaults(const TSLanguage *self, uint32_t index, TSSymbol symbol) {
  const uint16_t *descriptor = &self->small_parse_table[index & ~SMALL_STATE_DEFAULTS_FLAG];
  if (!ts_language_small_set_contains(self, ts_language__small_offset(descriptor), symbol)) return 0;
  uint16_t value = ts_language_lookup_small_compact(self, ts_language__small_offset(descriptor + 2), symbol);
  return value ? value : self->small_parse_table[symbol - self->token_count];
}

void ts_language_table_entry(const TSLanguage *self, TSStateId state, TSSymbol symbol, TableEntry *result);
TSLexerMode ts_language_lex_mode_for_state(const TSLanguage *self, TSStateId state);
bool ts_language_is_reserved_word(const TSLanguage *self, TSStateId state, TSSymbol symbol);
TSSymbolMetadata ts_language_symbol_metadata(const TSLanguage *self, TSSymbol symbol);
TSSymbol ts_language_public_symbol(const TSLanguage *self, TSSymbol symbol);
const TSLanguage *ts_language_copy_without_callbacks(const TSLanguage *self);
#ifdef __wasm__
uint32_t ts_language_current_context_id(void);
#endif

static inline const uint32_t *ts_language_actions(
  const TSLanguage *self,
  TSStateId state,
  TSSymbol symbol,
  uint32_t *count
) {
  TableEntry entry;
  ts_language_table_entry(self, state, symbol, &entry);
  *count = entry.action_count;
  return entry.actions;
}

static inline bool ts_parse_action_is_reduce(const TSParseAction *action) {
  return action->type == TSParseActionTypeReduce || action->type == TSParseActionTypeReduceExtra;
}

static inline bool ts_packed_parse_action_is_reduce(uint32_t action) {
  uint32_t type = action & TS_PARSE_ACTION_TYPE_MASK;
  return type == TSParseActionTypeReduce || type == TSParseActionTypeReduceExtra;
}

// Decode an action packed as `parser.h` describes.
static inline TSParseAction ts_language_parse_action(const TSLanguage *self, uint32_t action) {
  TSParseAction result = {.type = action & TS_PARSE_ACTION_TYPE_MASK};
  if (ts_packed_parse_action_is_reduce(action)) {
    const uint32_t *record = &self->parse_actions[(action >> 8) & 0xffff];
    result.reduce.symbol = record[0] & 0xffff;
    result.reduce.child_count = (record[0] >> 16) & 0xff;
    result.reduce.dynamic_precedence = (int16_t)(record[1] & 0xffff);
    result.reduce.production_id = record[1] >> 16;
  } else {
    result.shift.state = (action >> 8) & 0xffff;
    result.shift.extra = action & TS_PARSE_ACTION_EXTRA;
    result.shift.repetition = action & TS_PARSE_ACTION_REPETITION;
  }
  return result;
}

static inline bool ts_table_entry_has_reduce_action(const TableEntry *entry) {
  for (unsigned i = 0; i < entry->action_count; i++) {
    if (ts_packed_parse_action_is_reduce(entry->actions[i])) return true;
  }
  return false;
}

static inline bool ts_language_has_reduce_action(
  const TSLanguage *self,
  TSStateId state,
  TSSymbol symbol
) {
  TableEntry entry;
  ts_language_table_entry(self, state, symbol, &entry);
  return ts_table_entry_has_reduce_action(&entry);
}

// Lookup the table value for a given symbol and state.
//
// For non-terminal symbols, the table value represents a successor state.
// For terminal symbols, it represents an index in the actions table.
static inline uint16_t ts_language_lookup(
  const TSLanguage *self,
  TSStateId state,
  TSSymbol symbol
) {
  if (state >= self->large_state_count) {
    const uint32_t *map = &self->small_parse_table_map[2 * (state - self->large_state_count)];
    if (symbol < self->token_count) return ts_language_lookup_small_compact(self, map[0], symbol);
    uint16_t value = map[1] & SMALL_STATE_DEFAULTS_FLAG
      ? ts_language_lookup_goto_with_defaults(self, map[1], symbol)
      : ts_language_lookup_small_compact(self, map[1], symbol);
    if (value == UINT16_MAX && self->state_count <= UINT16_MAX) return state;
    return value;
  } else {
    return self->parse_table[state * self->symbol_count + symbol];
  }
}

static inline bool ts_language_has_actions(
  const TSLanguage *self,
  TSStateId state,
  TSSymbol symbol
) {
  return ts_language_lookup(self, state, symbol) != 0;
}

static inline void ts_lookahead_iterator__start_small_row(LookaheadIterator *self, bool is_goto_row) {
  const TSLanguage *language = self->language;
  uint32_t index = language->small_parse_table_map[2 * (self->state - language->large_state_count) + is_goto_row];
  self->is_goto_row = is_goto_row;
  self->uses_goto_defaults = index & SMALL_STATE_DEFAULTS_FLAG;
  self->first_symbol = is_goto_row ? language->token_count : 0;
  self->row_symbol_count = is_goto_row ? language->symbol_count - language->token_count : language->token_count;
  if (self->uses_goto_defaults) {
    // Visit the nonterminals of the state's goto set, looking each one's value up.
    const uint16_t *descriptor = &language->small_parse_table[index & ~SMALL_STATE_DEFAULTS_FLAG];
    const uint16_t *set = &language->small_parse_table[ts_language__small_offset(descriptor)];
    self->goto_row = ts_language__small_offset(descriptor + 2);
    self->set_symbol = set[0] * 16u - 16;
    self->set_data = set + 2;
    self->group_end = self->set_data + set[1];
    self->set_bits = 0;
    self->phase = LookaheadFresh;
    return;
  }
  uint32_t encoding = index & SMALL_STATE_SET_FLAG;
  uint32_t offset = index & ~SMALL_STATE_SET_FLAG;
  self->is_pair_state = encoding == SMALL_STATE_PAIR_FLAG;
  self->is_bitmap_state = encoding == SMALL_STATE_BITMAP_FLAG;
  self->is_set_state = encoding == SMALL_STATE_SET_FLAG;
  self->data = &language->small_parse_table[offset];
  self->group_end = self->is_set_state ? NULL : self->data + 1;
  self->group_count = *self->data;
  self->set_data = NULL;
  self->set_bits = 0;
  self->phase = LookaheadFresh;
  if (self->is_pair_state || self->is_set_state) self->data++;
}

// Iterate over all of the symbols that are valid in the given state.
//
// For 'large' parse states, this just requires iterating through
// all possible symbols and checking the parse table for each one.
// For 'small' parse states, this exploits the structure of the
// table to only visit the valid symbols.
static inline LookaheadIterator ts_language_lookaheads(
  const TSLanguage *self,
  TSStateId state
) {
  LookaheadIterator result = {
    .language = self,
    .is_small_state = state >= self->large_state_count,
    .state = state,
    .phase = LookaheadFresh,
    .symbol = UINT16_MAX,
    .next_state = 0,
  };
  if (result.is_small_state) {
    ts_lookahead_iterator__start_small_row(&result, false);
  } else {
    result.data = &self->parse_table[state * self->symbol_count];
  }
  return result;
}

static inline bool ts_lookahead_iterator__next_set(LookaheadIterator *self);
static inline bool ts_lookahead_iterator__next_bitmap(LookaheadIterator *self);
static inline bool ts_lookahead_iterator__next_group(LookaheadIterator *self);

static inline bool ts_lookahead_iterator__advance(LookaheadIterator *self);

static inline bool ts_lookahead_iterator__next(LookaheadIterator *self) {
  if (self->phase == LookaheadDone) return false;

  bool advanced = ts_lookahead_iterator__advance(self);
  if (!advanced && self->is_small_state && !self->is_goto_row) {
    ts_lookahead_iterator__start_small_row(self, true);
    advanced = ts_lookahead_iterator__advance(self);
  }
  if (!advanced) {
    self->phase = LookaheadDone;
    return false;
  }

  if (self->symbol < self->language->token_count) {
    self->actions = &self->language->parse_actions[self->table_value];
    self->action_count = *self->actions >> TS_PARSE_ACTION_COUNT_SHIFT;
    self->next_state = 0;
  } else {
    self->action_count = 0;
    self->next_state = self->table_value;
    if (self->is_small_state &&
        self->table_value == UINT16_MAX && self->language->state_count <= UINT16_MAX) {
      self->next_state = self->state;
    }
  }
  self->phase = LookaheadPositioned;
  return true;
}

static inline bool ts_lookahead_iterator__next_goto(LookaheadIterator *self);

static inline bool ts_lookahead_iterator__advance(LookaheadIterator *self) {
  bool advanced;
  if (self->uses_goto_defaults) {
    advanced = ts_lookahead_iterator__next_goto(self);
  } else if (self->is_set_state) {
    advanced = ts_lookahead_iterator__next_set(self);
  } else if (self->is_bitmap_state) {
    advanced = ts_lookahead_iterator__next_bitmap(self);
  } else if (self->is_pair_state) {
    advanced = self->group_count != 0;
    if (advanced) {
      self->group_count--;
      self->symbol = *(self->data++);
      self->table_value = *(self->data++);
    }
  } else if (self->is_small_state) {
    if (self->phase == LookaheadPositioned && self->data + 1 < self->group_end) {
      self->symbol = *(++self->data);
      return true;
    }
    advanced = ts_lookahead_iterator__next_group(self);
  } else {
    uint32_t symbol = self->phase == LookaheadFresh ? 0 : self->symbol + 1;
    while (symbol < self->language->symbol_count && !self->data[symbol]) symbol++;
    advanced = symbol < self->language->symbol_count;
    if (advanced) {
      self->symbol = symbol;
      self->table_value = self->data[symbol];
    }
  }
  return advanced;
}

static inline bool ts_lookahead_iterator__next_set(LookaheadIterator *self) {
  while (!self->set_bits) {
    if (self->set_data != self->group_end) {
      self->set_bits = *(self->set_data++);
      self->set_symbol += 16;
    } else if (self->group_count) {
      self->group_count--;
      self->table_value = *(self->data++);
      uint32_t offset = self->data[0] | ((uint32_t)self->data[1] << 16);
      self->data += 2;
      const uint16_t *set = &self->language->small_parse_table[offset];
      self->set_symbol = set[0] * 16u - 16;
      self->set_data = set + 2;
      self->group_end = self->set_data + set[1];
    } else {
      return false;
    }
  }
  self->symbol = self->set_symbol + ts_language__trailing_zeros(self->set_bits);
  self->set_bits &= self->set_bits - 1;
  return true;
}

static inline bool ts_lookahead_iterator__next_goto(LookaheadIterator *self) {
  while (!self->set_bits) {
    if (self->set_data == self->group_end) return false;
    self->set_bits = *(self->set_data++);
    self->set_symbol += 16;
  }
  const TSLanguage *language = self->language;
  self->symbol = self->set_symbol + ts_language__trailing_zeros(self->set_bits);
  self->set_bits &= self->set_bits - 1;
  self->table_value = ts_language_lookup_small_compact(language, self->goto_row, self->symbol);
  if (!self->table_value) self->table_value = language->small_parse_table[self->symbol - language->token_count];
  return true;
}

static inline bool ts_lookahead_iterator__next_bitmap(LookaheadIterator *self) {
  uint32_t symbol = self->phase == LookaheadFresh ? 0 : self->symbol + 1u - self->first_symbol;
  uint32_t blocks = (self->row_symbol_count + 15u) / 16;
  uint32_t block = symbol / 16;
  uint32_t bits = block < blocks ? self->data[block * 2] & (0xffffu << (symbol % 16)) : 0;
  while (!bits && ++block < blocks) bits = self->data[block * 2];
  if (block >= blocks) return false;
  uint32_t bit = ts_language__trailing_zeros(bits);
  self->symbol = self->first_symbol + block * 16 + bit;
  uint32_t rank = self->data[block * 2 + 1] + ts_language__count_ones(
    self->data[block * 2] & ((1u << bit) - 1)
  );
  self->table_value = self->data[blocks * 2 + rank];
  return true;
}

static inline bool ts_lookahead_iterator__next_group(LookaheadIterator *self) {
  if (self->group_count == 0) return false;
  self->data++;
  self->group_count--;
  self->table_value = *(self->data++);
  unsigned symbol_count = *(self->data++);
  self->group_end = self->data + symbol_count;
  self->symbol = *self->data;
  return true;
}

// Whether the state is a "primary state". If this returns false, it indicates that there exists
// another state that behaves identically to this one with respect to query analysis.
static inline bool ts_language_state_is_primary(
  const TSLanguage *self,
  TSStateId state
) {
  return state == self->primary_state_ids[state];
}

static inline uint32_t ts_language_external_state_stride(const TSLanguage *self) {
  return self->external_state_stride ? self->external_state_stride : self->external_token_count;
}

static inline const bool *ts_language_enabled_external_tokens(
  const TSLanguage *self,
  unsigned external_scanner_state
) {
  if (external_scanner_state == 0) {
    return NULL;
  } else {
    return self->external_scanner.states + ts_language_external_state_stride(self) * external_scanner_state;
  }
}

static inline uint32_t ts_language_alias_sequence_offset(const TSLanguage *self, uint32_t production_id) {
  if (self->alias_sequence_offsets) {
    return self->alias_sequence_offsets[production_id];
  }
  return production_id * self->max_alias_sequence_length;
}

static inline const TSSymbol *ts_language_alias_sequence(const TSLanguage *self, uint32_t production_id) {
  uint32_t offset = ts_language_alias_sequence_offset(self, production_id);
  return offset ? self->alias_sequences + offset : NULL;
}

static inline TSSymbol ts_language_alias_at(const TSLanguage *self, uint32_t production_id, uint32_t child_index) {
  uint32_t offset = ts_language_alias_sequence_offset(self, production_id);
  return offset ? self->alias_sequences[offset + child_index] : 0;
}

static inline void ts_language_field_map(
  const TSLanguage *self,
  uint32_t production_id,
  const TSFieldMapEntry **start,
  const TSFieldMapEntry **end
) {
  if (self->field_count == 0) {
    *start = NULL;
    *end = NULL;
    return;
  }

  TSMapSlice slice = self->field_map_slices[production_id];
  *start = &self->field_map_entries[slice.index];
  *end = &self->field_map_entries[slice.index] + slice.length;
}

static inline void ts_language_aliases_for_symbol(
  const TSLanguage *self,
  TSSymbol original_symbol,
  const TSSymbol **start,
  const TSSymbol **end
) {
  *start = &self->public_symbol_map[original_symbol];
  *end = *start + 1;

  unsigned idx = 0;
  for (;;) {
    TSSymbol symbol = self->alias_map[idx++];
    if (symbol == 0 || symbol > original_symbol) break;
    uint16_t count = self->alias_map[idx++];
    if (symbol == original_symbol) {
      *start = &self->alias_map[idx];
      *end = &self->alias_map[idx + count];
      break;
    }
    idx += count;
  }
}

static inline void ts_language_write_symbol_as_dot_string(
  const TSLanguage *self,
  FILE *f,
  TSSymbol symbol
) {
  const char *name = ts_language_symbol_name(self, symbol);
  for (const char *chr = name; *chr; chr++) {
    switch (*chr) {
      case '"':
      case '\\':
        fputc('\\', f);
        fputc(*chr, f);
        break;
      case '\n':
        fputs("\\n", f);
        break;
      case '\t':
        fputs("\\t", f);
        break;
      default:
        fputc(*chr, f);
        break;
    }
  }
}

static inline uint32_t ts_language__count_ones(uint32_t bits) {
#if defined(__clang__) || defined(__GNUC__)
  return __builtin_popcount(bits);
#else
  bits -= (bits >> 1) & 0x5555u;
  bits = (bits & 0x3333u) + ((bits >> 2) & 0x3333u);
  bits = (bits + (bits >> 4)) & 0x0f0fu;
  return ((bits * 0x0101u) >> 8) & 0x1fu;
#endif
}

static inline uint32_t ts_language__trailing_zeros(uint32_t bits) {
#if defined(__clang__) || defined(__GNUC__)
  return __builtin_ctz(bits);
#else
  uint32_t count = 0;
  while (!(bits & 1)) {
    bits >>= 1;
    count++;
  }
  return count;
#endif
}

#ifdef __cplusplus
}
#endif

#endif  // TREE_SITTER_LANGUAGE_H_
