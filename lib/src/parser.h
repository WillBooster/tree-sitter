#ifndef TREE_SITTER_PARSER_H_
#define TREE_SITTER_PARSER_H_

#ifdef __cplusplus
extern "C" {
#endif

#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

#define ts_builtin_sym_error ((TSSymbol)-1)
#define ts_builtin_sym_end 0
#define TREE_SITTER_SERIALIZATION_BUFFER_SIZE 1024

#ifndef TREE_SITTER_API_H_
typedef uint16_t TSStateId;
typedef uint16_t TSSymbol;
typedef uint16_t TSFieldId;
typedef struct TSLanguage TSLanguage;
typedef struct TSLanguageMetadata {
  uint8_t major_version;
  uint8_t minor_version;
  uint8_t patch_version;
} TSLanguageMetadata;
#endif

typedef struct {
  TSFieldId field_id;
  uint8_t child_index;
  bool inherited;
} TSFieldMapEntry;

// Used to index the field and supertype maps.
typedef struct {
  uint16_t index;
  uint16_t length;
} TSMapSlice;

typedef struct {
  bool visible;
  bool named;
  bool supertype;
  bool immediate;
} TSSymbolMetadata;

typedef struct TSLexer TSLexer;

typedef struct {
  uint32_t bytes;
  uint32_t row;
  uint32_t column;
} TSLexerPosition;

/**
 * Checkpoints may only restore marks during the lex invocation that captured them.
 * Their contents must not be inspected or serialized as scanner state.
 */
typedef struct {
  uint32_t _opaque[3];
} TSLexerCheckpoint;

struct TSLexer {
  int32_t lookahead;
  TSSymbol result_symbol;
  void (*advance)(TSLexer *, bool);
  void (*mark_end)(TSLexer *);
  uint32_t (*get_column)(TSLexer *);
  bool (*is_at_included_range_start)(const TSLexer *);
  bool (*eof)(const TSLexer *);
  void (*log)(const TSLexer *, const char *, ...);
  void (*get_position)(const TSLexer *, TSLexerPosition *);
  void (*mark_start)(TSLexer *, const TSLexerCheckpoint *);
  void (*mark_end_at)(TSLexer *, const TSLexerCheckpoint *);
  void (*get_mark_end)(const TSLexer *, TSLexerPosition *);
  uint32_t (*get_display_column)(TSLexer *, uint32_t tab_width);
  void (*get_checkpoint)(const TSLexer *, TSLexerCheckpoint *);
  void (*get_mark_end_checkpoint)(const TSLexer *, TSLexerCheckpoint *);
};

typedef enum {
  TSParseActionTypeShift,
  TSParseActionTypeReduce,
  TSParseActionTypeAccept,
  TSParseActionTypeRecover,
  TSParseActionTypeReduceExtra,
} TSParseActionType;

typedef union {
  struct {
    uint8_t type;
    TSStateId state;
    bool extra;
    bool repetition;
  } shift;
  struct {
    uint8_t type;
    uint8_t child_count;
    TSSymbol symbol;
    int16_t dynamic_precedence;
    uint16_t production_id;
  } reduce;
  uint8_t type;
} TSParseAction;

typedef struct {
  uint16_t lex_state;
  uint16_t lex_state_after_extra;
  uint16_t external_lex_state;
  uint16_t external_lex_state_after_extra;
  uint16_t reserved_word_set_id;
} TSLexerMode;

// `parse_actions` packs each action list into one word per action. Every word holds an action's type in bits
// 0-2, a shift's `extra` and `repetition` flags in bits 3 and 4, and in bits 8-23 a shift's state or the word
// index of a reduce's record. The first word of a list also holds the list's `reusable` flag in bit 24 and its
// action count in bits 25-31; an empty list is a single zero word. A reduce record is two words: the symbol and
// child count, then the dynamic precedence and production id.
#define TS_PARSE_ACTION_TYPE_MASK 0x7u
#define TS_PARSE_ACTION_EXTRA 0x8u
#define TS_PARSE_ACTION_REPETITION 0x10u
#define TS_PARSE_ACTION_MASK 0xffffffu
#define TS_PARSE_ACTION_REUSABLE 0x1000000u
#define TS_PARSE_ACTION_COUNT_SHIFT 25

typedef struct {
  int32_t start;
  int32_t end;
} TSCharacterRange;

#define TS_KEYWORD_PREFIX 0x10000u

struct TSLanguage {
  uint32_t abi_version;
  uint32_t symbol_count;
  uint32_t alias_count;
  uint32_t token_count;
  uint32_t external_token_count;
  uint32_t state_count;
  uint32_t large_state_count;
  uint32_t production_id_count;
  uint32_t field_count;
  uint16_t max_alias_sequence_length;
  const uint16_t *parse_table;
  const uint16_t *small_parse_table;
  const uint32_t *small_parse_table_map;
  const uint32_t *parse_actions;
  const char * const *symbol_names;
  const char * const *field_names;
  const TSMapSlice *field_map_slices;
  const TSFieldMapEntry *field_map_entries;
  const TSSymbolMetadata *symbol_metadata;
  const TSSymbol *public_symbol_map;
  const uint16_t *alias_map;
  const TSSymbol *alias_sequences;
  const TSLexerMode *lex_modes;
  bool (*lex_fn)(TSLexer *, TSStateId);
  bool (*keyword_lex_fn)(TSLexer *, TSStateId);
  TSSymbol keyword_capture_token;
  struct {
    const bool *states;
    const TSSymbol *symbol_map;
    void *(*create)(void);
    void (*destroy)(void *);
    bool (*scan)(void *, TSLexer *, const bool *symbol_whitelist);
    unsigned (*serialize)(void *, char *);
    void (*deserialize)(void *, const char *, unsigned);
  } external_scanner;
  const uint8_t *primary_state_bitmap;
  const char *name;
  const TSSymbol *reserved_words;
  uint16_t max_reserved_word_set_size;
  uint32_t supertype_count;
  const TSSymbol *supertype_symbols;
  const TSMapSlice *supertype_map_slices;
  const TSSymbol *supertype_map_entries;
  TSLanguageMetadata metadata;
  const uint32_t *alias_sequence_offsets;
  uint32_t alias_sequence_count;
  const void *lex_mode_ids;
  uint32_t lex_mode_count;
  const TSMapSlice *reserved_word_slices;
  uint32_t external_state_stride;
  uint32_t (*keyword_lookup_fn)(const char *, uint32_t);
};

static inline bool set_contains(const TSCharacterRange *ranges, uint32_t len, int32_t lookahead) {
  uint32_t index = 0;
  uint32_t size = len - index;
  while (size > 1) {
    uint32_t half_size = size / 2;
    uint32_t mid_index = index + half_size;
    const TSCharacterRange *range = &ranges[mid_index];
    if (lookahead >= range->start && lookahead <= range->end) {
      return true;
    } else if (lookahead > range->end) {
      index = mid_index;
    }
    size -= half_size;
  }
  const TSCharacterRange *range = &ranges[index];
  return (lookahead >= range->start && lookahead <= range->end);
}

/*
 *  Lexer Macros
 */

#ifdef _MSC_VER
#define UNUSED __pragma(warning(suppress : 4101))
#else
#define UNUSED __attribute__((unused))
#endif

#define START_LEXER()           \
  bool result = false;          \
  bool skip = false;            \
  UNUSED                        \
  bool eof = false;             \
  int32_t lookahead;            \
  goto start;                   \
  next_state:                   \
  lexer->advance(lexer, skip);  \
  start:                        \
  skip = false;                 \
  lookahead = lexer->lookahead;

#define ADVANCE(state_value) \
  {                          \
    state = state_value;     \
    goto next_state;         \
  }

#define ADVANCE_MAP(...)                                              \
  {                                                                   \
    static const uint16_t map[] = { __VA_ARGS__ };                    \
    for (uint32_t i = 0; i < sizeof(map) / sizeof(map[0]); i += 2) {  \
      if (map[i] == lookahead) {                                      \
        state = map[i + 1];                                           \
        goto next_state;                                              \
      }                                                               \
    }                                                                 \
  }

#define SKIP(state_value) \
  {                       \
    skip = true;          \
    state = state_value;  \
    goto next_state;      \
  }

#define ACCEPT_TOKEN(symbol_value)     \
  result = true;                       \
  lexer->result_symbol = symbol_value; \
  lexer->mark_end(lexer);

#define END_STATE() return result;

/*
 *  Parse Table Macros
 */

#define SMALL_STATE(id) ((id) - LARGE_STATE_COUNT)

#define STATE(id) id

#define ACTIONS(id) id

#define ACTION_LIST(count, reusable) \
  (((uint32_t)(count) << TS_PARSE_ACTION_COUNT_SHIFT) | ((reusable) ? TS_PARSE_ACTION_REUSABLE : 0))

#define SHIFT(state_value) (TSParseActionTypeShift | ((uint32_t)(state_value) << 8))

#define SHIFT_REPEAT(state_value) (SHIFT(state_value) | TS_PARSE_ACTION_REPETITION)

#define SHIFT_EXTRA() (TSParseActionTypeShift | TS_PARSE_ACTION_EXTRA)

#define REDUCE(record) (TSParseActionTypeReduce | ((uint32_t)(record) << 8))

#define REDUCE_EXTRA(record) (TSParseActionTypeReduceExtra | ((uint32_t)(record) << 8))

#define RECOVER() TSParseActionTypeRecover

#define ACCEPT_INPUT() TSParseActionTypeAccept

#define REDUCE_RECORD(symbol_name, children, precedence, prod_id)       \
  (uint32_t)(symbol_name) | ((uint32_t)(children) << 16),               \
  (uint32_t)(uint16_t)(precedence) | ((uint32_t)(prod_id) << 16)

#ifdef __cplusplus
}
#endif

#endif  // TREE_SITTER_PARSER_H_
