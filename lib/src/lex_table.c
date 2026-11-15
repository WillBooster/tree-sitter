#include "./lex_table.h"
#include "./alloc.h"
#include "./unicode.h"

#include <string.h>

#define TS_LEX_STACK_TAG_CAPACITY 16u

typedef struct {
  bool valid;
  int32_t precedence;
  TSSymbol symbol;
  TSLexerCheckpoint start;
  TSLexerCheckpoint end;
} LexFallback;

typedef struct {
  LexFallback fb;
  TSLexerCheckpoint *tags;
  bool result;
} LexRunState;

typedef struct {
  const uint8_t *ptr;
  uint32_t row;
  uint32_t col;
} LexFastPos;

static inline const TSLexTableView *ts_lex_select_view(Lexer *self, const uint16_t *table);
static bool ts_lex_run(Lexer *self, const TSLexTableView *v, uint16_t state, TSLexerCheckpoint *tags);
static inline bool ts_lex_can_run_utf8_fast(const Lexer *self, const TSLexTableView *v);
static bool ts_lex_utf8_ascii_fast(
  Lexer *self, const TSLexTableView *v, uint16_t *state_ptr, TSLexerCheckpoint *tags, bool *result
);
static inline uint16_t ts_lex_fast_action(
  const TSLexTableView *v, const uint16_t *sm, const uint8_t *ptr, const uint8_t *ptr_limit
);
static inline void ts_lex_record_simple_accept(
  Lexer *self, uint16_t acc, Length pos, const TSLexerCheckpoint *tags, bool *result
);
static inline void ts_lex_commit_fast_cursor(
  Lexer *self, Length pos, const uint8_t *ptr, const uint8_t *ptr_start, uint16_t *state_ptr, uint16_t state
);
static inline LexFastPos ts_lex_utf8_advance_run(
  const TSLexTableView *v, const uint16_t *sm, uint16_t state, uint16_t act, LexFastPos cur, const uint8_t *ptr_limit
);
static inline LexFastPos ts_lex_utf8_advance_masked(
  const TSLexTableView *v, const uint16_t *sm, uint16_t act, LexFastPos cur, const uint8_t *ptr_limit
);
static inline LexFastPos ts_lex_step_ascii(LexFastPos cur, uint8_t byte);
static inline void ts_lex_apply_accept(Lexer *self, const TSLexTableView *v, uint16_t acc, LexRunState *run);
static void ts_lex_apply_ext_accept(Lexer *self, const uint16_t *p, LexRunState *run);
static const uint16_t *ts_lex_apply_fallback_accept(Lexer *self, const uint16_t *p, LexRunState *run);
static inline uint16_t ts_lex_apply_transition(
  Lexer *self, const TSLexTableView *v, uint16_t act, TSLexerCheckpoint *tags
);
static inline bool ts_lex_can_lookup_keyword(const uint16_t *table, TSInputEncoding encoding);
static TSLexKeywordResult ts_lex_lookup_keyword_from(
  const TSLexTableView *v, const uint8_t *bytes, uint32_t byte_len, TSInputEncoding encoding,
  uint16_t state, bool is_keyword
);
static int ts_lex_lookup_keyword_utf8_prefix(
  const TSLexTableView *v, const uint8_t *bytes, uint32_t byte_len, uint32_t *offset, uint16_t *state, bool *is_keyword
);
static inline bool ts_lex_keyword_step(const TSLexTableView *v, uint16_t act, uint16_t *state);
static inline uint32_t ts_lex_decode_keyword_char(
  const uint8_t *bytes, uint32_t remaining, TSInputEncoding encoding, int32_t *cp
);
static inline uint16_t ts_lex_char_class(const TSLexTableView *v, int32_t lookahead, bool eof);
static inline uint16_t ts_lex_unicode_class(const TSLexTableView *v, uint32_t cp);
static inline uint16_t ts_lex_lookup_action(const TSLexTableView *v, TSLexStateMeta sm, uint16_t cls);

bool ts_lexer_lex_table(Lexer *self, const uint16_t *table, TSStateId start_state) {
  if (!table) return false;
  const TSLexTableView *v = ts_lex_select_view(self, table);
  if (start_state >= v->state_count) return false;

  TSLexerCheckpoint stack_tags[TS_LEX_STACK_TAG_CAPACITY];
  TSLexerCheckpoint *tags = NULL;
  if (v->tag_count > TS_LEX_STACK_TAG_CAPACITY) {
    tags = ts_calloc(v->tag_count, sizeof(TSLexerCheckpoint));
  } else if (v->tag_count > 0) {
    memset(stack_tags, 0, v->tag_count * sizeof(TSLexerCheckpoint));
    tags = stack_tags;
  }

  bool result = ts_lex_run(self, v, start_state, tags);
  if (tags && tags != stack_tags) ts_free(tags);
  return result;
}

TSLexKeywordResult ts_lexer_lookup_keyword(
  Lexer *self,
  const uint16_t *table,
  const uint8_t *bytes,
  uint32_t byte_len,
  TSInputEncoding encoding
) {
  if (!ts_lex_can_lookup_keyword(table, encoding)) return (TSLexKeywordResult){0, false, false};
  const TSLexTableView *v = ts_lex_select_view(self, table);
  uint16_t state = 0;
  bool is_keyword = false;
  uint32_t offset = 0;
  if (encoding == TSInputEncodingUTF8 && !v->wide_base) {
    int status = ts_lex_lookup_keyword_utf8_prefix(v, bytes, byte_len, &offset, &state, &is_keyword);
    if (status < 0) return (TSLexKeywordResult){0, false, false};
    if (status == 0) return (TSLexKeywordResult){0, true, is_keyword};
  }
  return ts_lex_lookup_keyword_from(v, bytes + offset, byte_len - offset, encoding, state, is_keyword);
}

static inline const TSLexTableView *ts_lex_select_view(Lexer *self, const uint16_t *table) {
  if (self->main_lex_cache.table == table) return &self->main_lex_cache;
  if (self->kw_lex_cache.table == table) return &self->kw_lex_cache;
  TSLexTableView *slot = !self->main_lex_cache.table ? &self->main_lex_cache : &self->kw_lex_cache;
  *slot = ts_lex_unpack_view(table);
  return slot;
}

static bool ts_lex_run(
  Lexer *self,
  const TSLexTableView *v,
  uint16_t state,
  TSLexerCheckpoint *tags
) {
  LexRunState run;
  run.fb.valid = false;
  run.tags = tags;
  run.result = false;
  uint32_t eof_steps = 0;
  for (;;) {
    if (ts_lex_can_run_utf8_fast(self, v) && ts_lex_utf8_ascii_fast(self, v, &state, tags, &run.result)) {
      return run.result;
    }
    TSLexStateMeta sm = ts_lex_state_meta(v, state);
    ts_lex_apply_accept(self, v, sm.acc, &run);
    bool eof = self->data.lookahead == 0 && self->data.eof(&self->data);
    uint16_t act = ts_lex_lookup_action(v, sm, ts_lex_char_class(v, self->data.lookahead, eof));
    if (act == 0 || (eof && ++eof_steps > v->state_count)) return run.result;
    state = ts_lex_apply_transition(self, v, act, tags);
  }
}

static inline bool ts_lex_can_run_utf8_fast(const Lexer *self, const TSLexTableView *v) {
  return !v->wide_base &&
    !self->track_columns &&
    self->input.encoding == TSInputEncodingUTF8 &&
    !self->logger.log &&
    self->current_position.bytes < self->ascii_limit &&
    (uint32_t)self->data.lookahead < 0x80u;
}

static bool ts_lex_utf8_ascii_fast(
  Lexer *self,
  const TSLexTableView *v,
  uint16_t *state_ptr,
  TSLexerCheckpoint *tags,
  bool *result
) {
  uint16_t state = *state_ptr;
  uint32_t start_bytes = self->current_position.bytes;
  const uint8_t *ptr_start = (const uint8_t *)self->chunk + (start_bytes - self->chunk_start);
  const uint8_t *ptr_limit = (const uint8_t *)self->chunk + (self->ascii_limit - self->chunk_start);
  LexFastPos cur = {ptr_start, self->current_position.extent.row, self->current_position.extent.column};
  for (;;) {
    const uint16_t *sm = &v->state_meta[(uint32_t)state * 4u];
    uint16_t acc = sm[3];
    uint16_t act = ts_lex_fast_action(v, sm, cur.ptr, ptr_limit);
    if ((acc | act) >= TS_LEX_EXTENDED_FLAG) break;
    Length pos = {start_bytes + (uint32_t)(cur.ptr - ptr_start), {cur.row, cur.col}};
    if (acc != 0) ts_lex_record_simple_accept(self, acc, pos, tags, result);
    if (act == 0) {
      ts_lex_commit_fast_cursor(self, pos, cur.ptr, ptr_start, state_ptr, state);
      return true;
    }
    if (act & TS_LEX_SIMPLE_TAG0_FLAG) tags[0] = (TSLexerCheckpoint){{pos.bytes, cur.row, cur.col}};
    cur = ts_lex_utf8_advance_run(v, sm, state, act, cur, ptr_limit);
    if (act & TS_LEX_SIMPLE_SKIP_FLAG) {
      self->token_start_position = (Length){start_bytes + (uint32_t)(cur.ptr - ptr_start), {cur.row, cur.col}};
    }
    state = (act & TS_LEX_SIMPLE_STATE_MASK) - 1u;
  }
  if (cur.ptr != ptr_start) {
    Length pos = {start_bytes + (uint32_t)(cur.ptr - ptr_start), {cur.row, cur.col}};
    ts_lex_commit_fast_cursor(self, pos, cur.ptr, ptr_start, state_ptr, state);
  }
  return false;
}

static inline uint16_t ts_lex_fast_action(
  const TSLexTableView *v,
  const uint16_t *sm,
  const uint8_t *ptr,
  const uint8_t *ptr_limit
) {
  uint16_t tmpl = sm[1];
  if (tmpl == 0) return 0;
  if (ptr >= ptr_limit || ptr[0] >= 0x80u) return UINT16_MAX;
  const uint16_t *ce = &v->comb[((uint32_t)sm[0] + v->ascii_class[ptr[0]]) * 2u];
  return ce[0] == tmpl ? ce[1] : sm[2];
}

static inline void ts_lex_record_simple_accept(
  Lexer *self,
  uint16_t acc,
  Length pos,
  const TSLexerCheckpoint *tags,
  bool *result
) {
  if (acc & TS_LEX_SIMPLE_ACCEPT_TAG0_FLAG) {
    Length start = {tags[0]._opaque[0], {tags[0]._opaque[1], tags[0]._opaque[2]}};
    self->token_start_position = start;
    self->marked_start_position = start;
  }
  *result = true;
  self->data.result_symbol = (TSSymbol)((acc & TS_LEX_SIMPLE_ACCEPT_MASK) - 1u);
  self->token_end_position = pos;
}

static inline void ts_lex_commit_fast_cursor(
  Lexer *self,
  Length pos,
  const uint8_t *ptr,
  const uint8_t *ptr_start,
  uint16_t *state_ptr,
  uint16_t state
) {
  *state_ptr = state;
  uint32_t advanced = (uint32_t)(ptr - ptr_start);
  if (advanced == 0) return;
  self->current_position = pos;
  self->advance_count += advanced;
  if (self->advance_count >= 64u && !ts_lexer_flush_operations(self)) return;
  if (!self->has_operation_limit && ptr[0] < 0x80u) {
    self->data.lookahead = ptr[0];
    self->lookahead_size = 1;
  } else {
    ts_lexer_get_lookahead(self);
  }
}

static inline LexFastPos ts_lex_utf8_advance_run(
  const TSLexTableView *v,
  const uint16_t *sm,
  uint16_t state,
  uint16_t act,
  LexFastPos cur,
  const uint8_t *ptr_limit
) {
  cur = ts_lex_step_ascii(cur, *cur.ptr++);
  if ((uint16_t)(act & ~TS_LEX_SIMPLE_SKIP_FLAG) != (uint16_t)(state + 1u)) return cur;
  const uint16_t *comb_row = &v->comb[(uint32_t)sm[0] * 2u];
  uint16_t tmpl = sm[1];
  uint16_t def = sm[2];
  const uint8_t *mask_trigger = cur.ptr + 32;
  while (cur.ptr < ptr_limit && cur.ptr < mask_trigger) {
    uint8_t b = *cur.ptr;
    if (b >= 0x80u) return cur;
    const uint16_t *ce = &comb_row[(uint32_t)v->ascii_class[b] * 2u];
    if ((ce[0] == tmpl ? ce[1] : def) != act) return cur;
    cur = ts_lex_step_ascii(cur, *cur.ptr++);
  }
  if (cur.ptr < ptr_limit) return ts_lex_utf8_advance_masked(v, sm, act, cur, ptr_limit);
  return cur;
}

static inline LexFastPos ts_lex_utf8_advance_masked(
  const TSLexTableView *v,
  const uint16_t *sm,
  uint16_t act,
  LexFastPos cur,
  const uint8_t *ptr_limit
) {
  const uint16_t *comb_row = &v->comb[(uint32_t)sm[0] * 2u];
  uint16_t tmpl = sm[1];
  uint16_t def = sm[2];
  uint64_t mask[2] = {0, 0};
  for (uint32_t c = 0; c < 128; c++) {
    const uint16_t *ce = &comb_row[(uint32_t)v->ascii_class[c] * 2u];
    if ((ce[0] == tmpl ? ce[1] : def) == act) mask[c >> 6] |= 1ull << (c & 63u);
  }
  while (cur.ptr < ptr_limit) {
    uint8_t b = *cur.ptr;
    if (b >= 0x80u || !((mask[b >> 6] >> (b & 63u)) & 1ull)) break;
    cur = ts_lex_step_ascii(cur, *cur.ptr++);
  }
  return cur;
}

static inline LexFastPos ts_lex_step_ascii(LexFastPos cur, uint8_t byte) {
  if (byte == '\n') {
    cur.row++;
    cur.col = 0;
  } else {
    cur.col++;
  }
  return cur;
}

static inline void ts_lex_apply_accept(
  Lexer *self,
  const TSLexTableView *v,
  uint16_t acc,
  LexRunState *run
) {
  if (acc == 0) return;
  if (acc < TS_LEX_EXTENDED_FLAG) {
    if (acc & TS_LEX_SIMPLE_ACCEPT_TAG0_FLAG) {
      self->data.mark_start(&self->data, &run->tags[0]);
    }
    run->result = true;
    self->data.result_symbol = (TSSymbol)((acc & TS_LEX_SIMPLE_ACCEPT_MASK) - 1u);
    self->data.mark_end(&self->data);
    return;
  }
  ts_lex_apply_ext_accept(self, &v->ext_accepts[acc & 0x7fffu], run);
}

static void ts_lex_apply_ext_accept(
  Lexer *self,
  const uint16_t *p,
  LexRunState *run
) {
  uint16_t flags = *p++;
  if (flags & 1u) p = ts_lex_apply_fallback_accept(self, p, run);
  if (flags & 2u) {
    run->result = run->fb.valid;
    if (run->result) {
      self->data.result_symbol = run->fb.symbol;
      self->data.mark_start(&self->data, &run->fb.start);
      self->data.mark_end_at(&self->data, &run->fb.end);
    }
    return;
  }
  if (!(flags & 4u)) return;
  TSSymbol sym = *p++;
  bool allowed = true;
  if (flags & 8u) {
    int32_t prec = (int32_t)ts_lex_read_u32(p);
    p += 2;
    allowed = !run->fb.valid || prec >= run->fb.precedence;
  }
  uint16_t tag = (flags & 16u) ? *p : UINT16_MAX;
  if (!allowed) return;
  if (tag != UINT16_MAX) self->data.mark_start(&self->data, &run->tags[tag]);
  run->result = true;
  self->data.result_symbol = sym;
  self->data.mark_end(&self->data);
}

static const uint16_t *ts_lex_apply_fallback_accept(
  Lexer *self,
  const uint16_t *p,
  LexRunState *run
) {
  TSSymbol sym = *p++;
  int32_t prec = (int32_t)ts_lex_read_u32(p);
  uint16_t tag = p[2];
  if (!run->fb.valid || prec >= run->fb.precedence) {
    run->fb.valid = true;
    run->fb.precedence = prec;
    run->fb.symbol = sym;
    if (tag != UINT16_MAX) {
      run->fb.start = run->tags[tag];
    } else {
      self->data.get_checkpoint(&self->data, &run->fb.start);
    }
    self->data.mark_end(&self->data);
    self->data.get_mark_end_checkpoint(&self->data, &run->fb.end);
  }
  return p + 3;
}

static inline uint16_t ts_lex_apply_transition(
  Lexer *self,
  const TSLexTableView *v,
  uint16_t act,
  TSLexerCheckpoint *tags
) {
  if (act < TS_LEX_EXTENDED_FLAG) {
    if (act & TS_LEX_SIMPLE_TAG0_FLAG) self->data.get_checkpoint(&self->data, &tags[0]);
    self->data.advance(&self->data, (act & TS_LEX_SIMPLE_SKIP_FLAG) != 0);
    return (uint16_t)((act & TS_LEX_SIMPLE_STATE_MASK) - 1u);
  }
  const uint16_t *p = &v->ext_transitions[act & 0x7fffu];
  uint16_t next_state = p[0];
  bool skip = (p[1] & TS_LEX_EXTENDED_FLAG) != 0;
  uint16_t op_count = p[1] & 0x7fffu;
  p += 2;
  for (uint16_t i = 0; i < op_count; i++, p += 2) {
    if (p[1] != UINT16_MAX) {
      tags[p[0]] = tags[p[1]];
    } else {
      self->data.get_checkpoint(&self->data, &tags[p[0]]);
    }
  }
  self->data.advance(&self->data, skip);
  return next_state;
}

static inline bool ts_lex_can_lookup_keyword(const uint16_t *table, TSInputEncoding encoding) {
  return table &&
    (table[6] & TS_LEX_DIRECT_KEYWORD_FLAG) &&
    (encoding == TSInputEncodingUTF8 ||
     encoding == TSInputEncodingUTF16LE ||
     encoding == TSInputEncodingUTF16BE);
}

static TSLexKeywordResult ts_lex_lookup_keyword_from(
  const TSLexTableView *v,
  const uint8_t *bytes,
  uint32_t byte_len,
  TSInputEncoding encoding,
  uint16_t state,
  bool is_keyword
) {
  uint32_t offset = 0;
  while (offset < byte_len) {
    TSLexStateMeta sm = ts_lex_state_meta(v, state);
    if (sm.acc != 0) is_keyword = true;
    int32_t cp = 0;
    uint32_t step = ts_lex_decode_keyword_char(bytes + offset, byte_len - offset, encoding, &cp);
    if (step == 0) break;
    offset += step;
    uint16_t act = ts_lex_lookup_action(v, sm, ts_lex_char_class(v, cp, false));
    if (act == 0) return (TSLexKeywordResult){0, true, is_keyword};
    if (!ts_lex_keyword_step(v, act, &state)) return (TSLexKeywordResult){0, false, false};
  }
  uint16_t acc = ts_lex_state_meta(v, state).acc;
  if (acc != 0) return (TSLexKeywordResult){(TSSymbol)((acc & TS_LEX_SIMPLE_ACCEPT_MASK) - 1u), true, true};
  return (TSLexKeywordResult){0, true, is_keyword};
}

static int ts_lex_lookup_keyword_utf8_prefix(
  const TSLexTableView *v,
  const uint8_t *bytes,
  uint32_t byte_len,
  uint32_t *offset,
  uint16_t *state,
  bool *is_keyword
) {
  uint32_t idx = *offset;
  uint16_t cur_state = *state;
  bool kw = *is_keyword;
  while (idx < byte_len && bytes[idx] < 0x80u) {
    const uint16_t *sm = &v->state_meta[(uint32_t)cur_state * 4u];
    if (sm[3] != 0) kw = true;
    uint16_t tmpl = sm[1];
    if (tmpl == 0) {
      *is_keyword = kw;
      return 0;
    }
    const uint16_t *ce = &v->comb[((uint32_t)sm[0] + v->ascii_class[bytes[idx]]) * 2u];
    uint16_t act = ce[0] == tmpl ? ce[1] : sm[2];
    if (act == 0) {
      *is_keyword = kw;
      return 0;
    }
    if (!ts_lex_keyword_step(v, act, &cur_state)) return -1;
    idx++;
  }
  *offset = idx;
  *state = cur_state;
  *is_keyword = kw;
  return 1;
}

static inline bool ts_lex_keyword_step(const TSLexTableView *v, uint16_t act, uint16_t *state) {
  if (act < TS_LEX_EXTENDED_FLAG) {
    if (act & TS_LEX_SIMPLE_SKIP_FLAG) return false;
    *state = (uint16_t)((act & TS_LEX_SIMPLE_STATE_MASK) - 1u);
    return true;
  }
  const uint16_t *p = &v->ext_transitions[act & 0x7fffu];
  if (p[1] & TS_LEX_EXTENDED_FLAG) return false;
  *state = p[0];
  return true;
}

static inline uint32_t ts_lex_decode_keyword_char(
  const uint8_t *bytes,
  uint32_t remaining,
  TSInputEncoding encoding,
  int32_t *cp
) {
  if (encoding == TSInputEncodingUTF8 && bytes[0] < 0x80u) {
    *cp = bytes[0];
    return 1;
  }
  if (encoding == TSInputEncodingUTF16LE && remaining >= 2 && bytes[0] < 0x80u && bytes[1] == 0) {
    *cp = bytes[0];
    return 2;
  }
  uint32_t step = encoding == TSInputEncodingUTF8 ? ts_decode_utf8(bytes, remaining, cp)
    : encoding == TSInputEncodingUTF16LE ? ts_decode_utf16_le(bytes, remaining, cp)
    : ts_decode_utf16_be(bytes, remaining, cp);
  return *cp == TS_DECODE_ERROR ? 1 : step;
}

static inline uint16_t ts_lex_char_class(const TSLexTableView *v, int32_t lookahead, bool eof) {
  if (eof) return 0;
  uint32_t cp = (uint32_t)lookahead;
  if (cp < 0x80u) return v->ascii_class[cp];
  if (cp < 0x110000u) return ts_lex_unicode_class(v, cp);
  return 1;
}

static inline uint16_t ts_lex_unicode_class(const TSLexTableView *v, uint32_t cp) {
  uint32_t low = 0;
  uint32_t high = v->unicode_range_count - 1u;
  while (low < high) {
    uint32_t mid = low + (high - low) / 2u;
    if (cp > ts_lex_read_u32(&v->unicode_ends[mid * 2u])) {
      low = mid + 1u;
    } else {
      high = mid;
    }
  }
  return v->unicode_classes[low];
}

static inline uint16_t ts_lex_lookup_action(
  const TSLexTableView *v,
  TSLexStateMeta sm,
  uint16_t cls
) {
  if (sm.tmpl == 0) return 0;
  const uint16_t *ce = &v->comb[(sm.base + cls) * 2u];
  return ce[0] == sm.tmpl ? ce[1] : sm.def;
}
