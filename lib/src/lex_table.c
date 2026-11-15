// code-gauge-allow-file fileNcss 463 -- keeps DFA execution, Unicode exception bounds, and accept guards together
#include "./lex_table.h"
#include "./unicode.h"

#include <string.h>

#define TS_LEX_STACK_TAG_CAPACITY 16u

typedef struct {
  bool valid;
  bool has_decode_error;
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
  uint32_t steps;
} LexFastPos;

typedef struct {
  int32_t cp;
  uint32_t step;
  uint16_t act;
} LexFastNext;

typedef struct {
  Lexer *self;
  TSLexTableView *v;
  TSLexerCheckpoint *tags;
  const uint8_t *ptr_start;
  const uint8_t *ptr_limit;
  uint32_t start_bytes;
  bool utf16;
} LexFastCtx;

typedef struct {
  TSLexUnicodeExc exc;
  bool exc_loaded;
  uint32_t hit_lo;
  uint32_t hit_span;
  uint16_t hit_act;
} LexUnicodeLoop;

typedef struct {
  uint32_t *cls_lo;
  uint32_t *cls_hi;
  uint32_t *min_lo;
  uint32_t *max_hi;
  uint32_t *tmpl_base;
  uint16_t *tmpl_def;
} LexUnicodeExcScratch;

static bool ts_lex_run(Lexer *self, TSLexTableView *v, uint16_t state, TSLexerCheckpoint *tags);
static inline uint16_t ts_lex_next_action(
  Lexer *self, const TSLexTableView *v, TSLexStateMeta sm, bool eof
);
static inline bool ts_lex_action_is_skip(const TSLexTableView *v, uint16_t act);
static inline bool ts_lex_can_run_fast(const Lexer *self, const TSLexTableView *v);
static bool ts_lex_fast(
  Lexer *self, TSLexTableView *v, uint16_t *state_ptr, TSLexerCheckpoint *tags, bool *result
);
static inline bool ts_lex_fast_loop(
  LexFastCtx *ctx, bool utf16, uint16_t *state_ptr, bool *result
);
static inline LexFastPos ts_lex_fast_step(
  LexFastCtx *ctx, const uint16_t *sm, uint16_t state, LexFastNext next, LexFastPos cur
);
static inline LexFastNext ts_lex_fast_next(
  TSLexTableView *v, const uint16_t *sm, const uint8_t *ptr, const uint8_t *ptr_limit, bool utf16
);
static uint16_t ts_lex_unicode_action(TSLexTableView *v, const uint16_t *sm, uint32_t cp);
static inline void ts_lex_record_simple_accept(
  Lexer *self, uint16_t acc, Length pos, const TSLexerCheckpoint *tags, bool *result
);
static inline void ts_lex_commit_fast_cursor(const LexFastCtx *ctx, Length pos, LexFastPos cur);
static inline LexFastPos ts_lex_advance_self_loop(
  const LexFastCtx *ctx, const uint16_t *sm, uint16_t act, LexFastPos cur
);
static inline uint32_t ts_lex_self_loop_unicode(
  const LexFastCtx *ctx, const uint16_t *sm, uint16_t act, const uint8_t *ptr, LexUnicodeLoop *uni
);
static inline LexFastPos ts_lex_step_pos(LexFastPos cur, int32_t cp, uint32_t step);
static inline uint32_t ts_lex_decode_fast(
  const uint8_t *ptr, const uint8_t *ptr_limit, bool utf16, int32_t *cp_out
);
static inline uint32_t ts_lex_decode_utf16le_surrogate(
  const uint8_t *ptr, const uint8_t *ptr_limit, uint16_t u, int32_t *cp_out
);
static inline const TSLexUnicodeExc *ts_lex_ensure_unicode_exc(TSLexTableView *v);
static void ts_lex_build_unicode_exc(TSLexTableView *v);
static void ts_lex_fill_unicode_class_bounds(const TSLexTableView *v, uint32_t *cls_lo, uint32_t *cls_hi);
static void ts_lex_collect_template_meta(const TSLexTableView *v, uint32_t *tmpl_base, uint16_t *tmpl_def);
static void ts_lex_accumulate_comb_exceptions(const TSLexTableView *v, const LexUnicodeExcScratch *s);
static inline void ts_lex_widen_bounds(uint32_t *min_lo, uint32_t *max_hi, uint32_t lo, uint32_t hi);
static inline void ts_lex_apply_accept(Lexer *self, const TSLexTableView *v, uint16_t acc, LexRunState *run);
static void ts_lex_apply_ext_accept(Lexer *self, const uint16_t *p, LexRunState *run);
static inline void ts_lex_restore_fallback_accept(Lexer *self, LexRunState *run);
static const uint16_t *ts_lex_apply_fallback_accept(Lexer *self, const uint16_t *p, LexRunState *run);
static inline bool ts_lex_check_accept_decode_error(
  const Lexer *self, uint32_t accept_start_byte, bool *out_has_decode_error
);
static inline uint16_t ts_lex_apply_transition(
  Lexer *self, const TSLexTableView *v, uint16_t act, TSLexerCheckpoint *tags
);

bool ts_lexer_lex_table(Lexer *self, const uint16_t *table, TSStateId start_state) {
  if (!table) return false;
  TSLexTableView *v = ts_lex_select_view(self, table);
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

static bool ts_lex_run(
  Lexer *self,
  TSLexTableView *v,
  uint16_t state,
  TSLexerCheckpoint *tags
) {
  LexRunState run;
  run.fb.valid = false;
  run.fb.has_decode_error = false;
  run.tags = tags;
  run.result = false;
  uint32_t eof_steps = 0;
  for (;;) {
    if (ts_lex_can_run_fast(self, v) && ts_lex_fast(self, v, &state, tags, &run.result)) {
      return run.result;
    }
    TSLexStateMeta sm = ts_lex_state_meta(v, state);
    ts_lex_apply_accept(self, v, sm.acc, &run);
    bool eof = self->data.lookahead == 0 && self->data.eof(&self->data);
    uint16_t act = ts_lex_next_action(self, v, sm, eof);
    if (act == 0 || (eof && ++eof_steps > v->state_count)) return run.result;
    state = ts_lex_apply_transition(self, v, act, tags);
  }
}

static inline uint16_t ts_lex_next_action(
  Lexer *self,
  const TSLexTableView *v,
  TSLexStateMeta sm,
  bool eof
) {
  if (!eof && (uint32_t)self->data.lookahead >= 0x110000u) {
    uint16_t act = ts_lex_lookup_action(v, sm, v->replacement_class);
    if (ts_lex_action_is_skip(v, act)) return 0;
    if (act != 0 && !self->has_decode_error) {
      self->has_decode_error = true;
      self->first_decode_error_byte = self->current_position.bytes;
    }
    return act;
  }
  return ts_lex_lookup_action(v, sm, ts_lex_char_class(v, self->data.lookahead, eof));
}

static inline bool ts_lex_action_is_skip(const TSLexTableView *v, uint16_t act) {
  if (act == 0) return false;
  if (act < TS_LEX_EXTENDED_FLAG) return (act & TS_LEX_SIMPLE_SKIP_FLAG) != 0;
  return (v->ext_transitions[(act & 0x7fffu) + 1u] & TS_LEX_EXTENDED_FLAG) != 0;
}

static inline bool ts_lex_can_run_fast(const Lexer *self, const TSLexTableView *v) {
  return !v->wide_base &&
    !self->track_columns &&
    (self->input.encoding == TSInputEncodingUTF8 ||
     self->input.encoding == TSInputEncodingUTF16LE) &&
    !self->logger.log &&
    self->current_position.bytes < self->ascii_limit &&
    (uint32_t)self->data.lookahead < 0x110000u;
}

static bool ts_lex_fast(
  Lexer *self,
  TSLexTableView *v,
  uint16_t *state_ptr,
  TSLexerCheckpoint *tags,
  bool *result
) {
  uint32_t start_bytes = self->current_position.bytes;
  LexFastCtx ctx = {
    .self = self,
    .v = v,
    .tags = tags,
    .ptr_start = (const uint8_t *)self->chunk + (start_bytes - self->chunk_start),
    .ptr_limit = (const uint8_t *)self->chunk + (self->ascii_limit - self->chunk_start),
    .start_bytes = start_bytes,
    .utf16 = self->input.encoding == TSInputEncodingUTF16LE,
  };
  return ctx.utf16
    ? ts_lex_fast_loop(&ctx, true, state_ptr, result)
    : ts_lex_fast_loop(&ctx, false, state_ptr, result);
}

static inline bool ts_lex_fast_loop(
  LexFastCtx *ctx,
  bool utf16,
  uint16_t *state_ptr,
  bool *result
) {
  Lexer *self = ctx->self;
  TSLexTableView *v = ctx->v;
  uint16_t state = *state_ptr;
  LexFastPos cur = {
    ctx->ptr_start,
    self->current_position.extent.row,
    self->current_position.extent.column,
    0,
  };
  for (;;) {
    const uint16_t *sm = &v->state_meta[(uint32_t)state * 4u];
    uint16_t acc = sm[3];
    LexFastNext next = ts_lex_fast_next(v, sm, cur.ptr, ctx->ptr_limit, utf16);
    if ((acc | next.act) >= TS_LEX_EXTENDED_FLAG) break;
    Length pos = {ctx->start_bytes + (uint32_t)(cur.ptr - ctx->ptr_start), {cur.row, cur.col}};
    if (acc != 0) ts_lex_record_simple_accept(self, acc, pos, ctx->tags, result);
    if (next.act == 0) {
      *state_ptr = state;
      if (cur.steps != 0) ts_lex_commit_fast_cursor(ctx, pos, cur);
      return true;
    }
    cur = ts_lex_fast_step(ctx, sm, state, next, cur);
    state = (next.act & TS_LEX_SIMPLE_STATE_MASK) - 1u;
  }
  if (cur.steps != 0) {
    *state_ptr = state;
    Length pos = {ctx->start_bytes + (uint32_t)(cur.ptr - ctx->ptr_start), {cur.row, cur.col}};
    ts_lex_commit_fast_cursor(ctx, pos, cur);
  }
  return false;
}

static inline LexFastPos ts_lex_fast_step(
  LexFastCtx *ctx,
  const uint16_t *sm,
  uint16_t state,
  LexFastNext next,
  LexFastPos cur
) {
  uint16_t act = next.act;
  if (act & TS_LEX_SIMPLE_TAG0_FLAG) {
    uint32_t byte = ctx->start_bytes + (uint32_t)(cur.ptr - ctx->ptr_start);
    ctx->tags[0] = (TSLexerCheckpoint){{byte, cur.row, cur.col}};
  }
  cur = ts_lex_step_pos(cur, next.cp, next.step);
  if ((uint16_t)(act & ~TS_LEX_SIMPLE_SKIP_FLAG) == (uint16_t)(state + 1u)) {
    cur = ts_lex_advance_self_loop(ctx, sm, act, cur);
  }
  if (act & TS_LEX_SIMPLE_SKIP_FLAG) {
    uint32_t byte = ctx->start_bytes + (uint32_t)(cur.ptr - ctx->ptr_start);
    ctx->self->token_start_position = (Length){byte, {cur.row, cur.col}};
  }
  return cur;
}

static inline LexFastNext ts_lex_fast_next(
  TSLexTableView *v,
  const uint16_t *sm,
  const uint8_t *ptr,
  const uint8_t *ptr_limit,
  bool utf16
) {
  uint16_t tmpl = sm[1];
  if (tmpl == 0) return (LexFastNext){0, 0, 0};
  if (ptr >= ptr_limit) return (LexFastNext){0, 0, UINT16_MAX};
  int32_t cp = 0;
  uint32_t step = ts_lex_decode_fast(ptr, ptr_limit, utf16, &cp);
  if (!step) return (LexFastNext){0, 0, UINT16_MAX};
  if ((uint32_t)cp < 0x80u) {
    const uint16_t *ce = &v->comb[((uint32_t)sm[0] + v->ascii_class[(uint8_t)cp]) * 2u];
    return (LexFastNext){cp, step, ce[0] == tmpl ? ce[1] : sm[2]};
  }
  return (LexFastNext){cp, step, ts_lex_unicode_action(v, sm, (uint32_t)cp)};
}

static uint16_t ts_lex_unicode_action(TSLexTableView *v, const uint16_t *sm, uint32_t cp) {
  uint16_t tmpl = sm[1];
  TSLexUnicodeExc exc = ts_lex_ensure_unicode_exc(v)[tmpl];
  if (cp - exc.lo > exc.span) return sm[2];
  uint16_t cls = ts_lex_unicode_class(v, cp);
  const uint16_t *ce = &v->comb[((uint32_t)sm[0] + cls) * 2u];
  return ce[0] == tmpl ? ce[1] : sm[2];
}

static inline void ts_lex_record_simple_accept(
  Lexer *self,
  uint16_t acc,
  Length pos,
  const TSLexerCheckpoint *tags,
  bool *result
) {
  uint32_t start_byte = (acc & TS_LEX_SIMPLE_ACCEPT_TAG0_FLAG)
    ? tags[0]._opaque[0]
    : self->token_start_position.bytes;
  bool has_err = false;
  if (!ts_lex_check_accept_decode_error(self, start_byte, &has_err)) return;
  if (acc & TS_LEX_SIMPLE_ACCEPT_TAG0_FLAG) {
    Length start = {tags[0]._opaque[0], {tags[0]._opaque[1], tags[0]._opaque[2]}};
    self->token_start_position = start;
    self->marked_start_position = start;
  }
  *result = true;
  self->data.result_symbol = (TSSymbol)((acc & TS_LEX_SIMPLE_ACCEPT_MASK) - 1u);
  self->token_end_position = pos;
  self->token_has_decode_error = has_err;
}

static inline void ts_lex_commit_fast_cursor(
  const LexFastCtx *ctx,
  Length pos,
  LexFastPos cur
) {
  Lexer *self = ctx->self;
  self->current_position = pos;
  self->advance_count += cur.steps;
  if (self->advance_count >= 64u && !ts_lexer_flush_operations(self)) return;
  if (!self->has_operation_limit) {
    int32_t cp = 0;
    uint32_t step = cur.ptr < ctx->ptr_limit
      ? ts_lex_decode_fast(cur.ptr, ctx->ptr_limit, ctx->utf16, &cp)
      : (!ctx->utf16 && cur.ptr[0] < 0x80u ? (cp = cur.ptr[0], 1u) : 0u);
    if (step != 0) {
      self->data.lookahead = cp;
      self->lookahead_size = step;
      return;
    }
  }
  ts_lexer_get_lookahead(self);
}

static inline uint16_t ts_lex_ascii_comb_act(
  const TSLexTableView *v,
  const uint16_t *comb_row,
  uint16_t tmpl,
  uint16_t def,
  uint16_t u
) {
  const uint16_t *ce = &comb_row[(uint32_t)v->ascii_class[u] * 2u];
  return ce[0] == tmpl ? ce[1] : def;
}

static inline LexFastPos ts_lex_advance_self_loop(
  const LexFastCtx *ctx,
  const uint16_t *sm,
  uint16_t act,
  LexFastPos cur
) {
  TSLexTableView *v = ctx->v;
  const uint16_t *comb_row = &v->comb[(uint32_t)sm[0] * 2u];
  uint16_t tmpl = sm[1];
  uint16_t def = sm[2];
  uint32_t ascii_step = ctx->utf16 ? 2u : 1u;
  LexUnicodeLoop uni = {
    .exc = v->unicode_exc ? v->unicode_exc[tmpl] : (TSLexUnicodeExc){0, 0},
    .exc_loaded = v->unicode_exc != NULL,
    .hit_lo = 0x110000u,
    .hit_span = 0u,
    .hit_act = 0,
  };
  while (cur.ptr < ctx->ptr_limit) {
    uint16_t u = ctx->utf16
      ? ((uint16_t)cur.ptr[0] | ((uint16_t)cur.ptr[1] << 8))
      : cur.ptr[0];
    if (u < 0x80u) {
      if (ts_lex_ascii_comb_act(v, comb_row, tmpl, def, u) != act) break;
      cur = ts_lex_step_pos(cur, u, ascii_step);
    } else {
      uint32_t step = ts_lex_self_loop_unicode(ctx, sm, act, cur.ptr, &uni);
      if (!step) break;
      cur.ptr += step;
      cur.col += step;
      cur.steps++;
    }
  }
  return cur;
}

static inline uint32_t ts_lex_self_loop_unicode(
  const LexFastCtx *ctx,
  const uint16_t *sm,
  uint16_t act,
  const uint8_t *ptr,
  LexUnicodeLoop *uni
) {
  int32_t cp = 0;
  uint32_t step = ts_lex_decode_fast(ptr, ctx->ptr_limit, ctx->utf16, &cp);
  if (!step) return 0u;
  if (!uni->exc_loaded) {
    uni->exc = ts_lex_ensure_unicode_exc(ctx->v)[sm[1]];
    uni->exc_loaded = true;
  }
  uint16_t next_act;
  if ((uint32_t)cp - uni->exc.lo > uni->exc.span) {
    next_act = sm[2];
  } else if ((uint32_t)cp - uni->hit_lo <= uni->hit_span) {
    next_act = uni->hit_act;
  } else {
    uint16_t cls = ts_lex_unicode_range(ctx->v, (uint32_t)cp, &uni->hit_lo, &uni->hit_span);
    const uint16_t *ce = &ctx->v->comb[((uint32_t)sm[0] + cls) * 2u];
    next_act = ce[0] == sm[1] ? ce[1] : sm[2];
    uni->hit_act = next_act;
  }
  return next_act == act ? step : 0u;
}

static inline LexFastPos ts_lex_step_pos(LexFastPos cur, int32_t cp, uint32_t step) {
  if (cp == '\n') {
    cur.row++;
    cur.col = 0;
  } else {
    cur.col += step;
  }
  cur.ptr += step;
  cur.steps++;
  return cur;
}

static inline uint32_t ts_lex_decode_fast(
  const uint8_t *ptr,
  const uint8_t *ptr_limit,
  bool utf16,
  int32_t *cp_out
) {
  if (!utf16) {
    if (ptr[0] < 0x80u) {
      *cp_out = ptr[0];
      return 1u;
    }
    if ((size_t)(ptr_limit - ptr) < 4u) return 0u;
    uint32_t step = ts_decode_utf8(ptr, 4u, cp_out);
    return *cp_out >= 0 ? step : 0u;
  }
  uint16_t u = (uint16_t)ptr[0] | ((uint16_t)ptr[1] << 8);
  if ((u & 0xf800u) != 0xd800u) {
    *cp_out = u;
    return 2u;
  }
  return ts_lex_decode_utf16le_surrogate(ptr, ptr_limit, u, cp_out);
}

static inline uint32_t ts_lex_decode_utf16le_surrogate(
  const uint8_t *ptr,
  const uint8_t *ptr_limit,
  uint16_t u,
  int32_t *cp_out
) {
  if ((size_t)(ptr_limit - ptr) < 4u) return 0u;
  if (U16_IS_LEAD(u)) {
    uint16_t u2 = (uint16_t)ptr[2] | ((uint16_t)ptr[3] << 8);
    if (U16_IS_TRAIL(u2)) {
      *cp_out = U16_GET_SUPPLEMENTARY(u, u2);
      return 4u;
    }
  }
  *cp_out = u;
  return 2u;
}

static inline const TSLexUnicodeExc *ts_lex_ensure_unicode_exc(TSLexTableView *v) {
  if (!v->unicode_exc) ts_lex_build_unicode_exc(v);
  return v->unicode_exc;
}

static void ts_lex_build_unicode_exc(TSLexTableView *v) {
  uint32_t tmpl_n = v->template_count;
  uint32_t cls_n = v->class_count;
  TSLexUnicodeExc *exc = ts_malloc((size_t)tmpl_n * sizeof(TSLexUnicodeExc));
  size_t scratch_bytes = ((size_t)cls_n * 2u + (size_t)tmpl_n * 4u) * sizeof(uint32_t);
  uint32_t *buf = ts_malloc(scratch_bytes);
  LexUnicodeExcScratch s = {
    .cls_lo = buf,
    .cls_hi = buf + cls_n,
    .min_lo = buf + cls_n * 2u,
    .max_hi = buf + cls_n * 2u + tmpl_n,
    .tmpl_base = buf + cls_n * 2u + tmpl_n * 2u,
    .tmpl_def = (uint16_t *)(buf + cls_n * 2u + tmpl_n * 3u),
  };

  ts_lex_fill_unicode_class_bounds(v, s.cls_lo, s.cls_hi);
  for (uint32_t t = 0; t < tmpl_n; t++) {
    s.min_lo[t] = 0x110000u;
    s.max_hi[t] = 0u;
    s.tmpl_base[t] = UINT32_MAX;
    s.tmpl_def[t] = 0;
  }
  ts_lex_collect_template_meta(v, s.tmpl_base, s.tmpl_def);
  ts_lex_accumulate_comb_exceptions(v, &s);

  for (uint32_t t = 0; t < tmpl_n; t++) {
    exc[t] = s.min_lo[t] <= s.max_hi[t]
      ? (TSLexUnicodeExc){s.min_lo[t], s.max_hi[t] - s.min_lo[t]}
      : (TSLexUnicodeExc){0x110000u, 0u};
  }
  ts_free(buf);
  v->unicode_exc = exc;
}

static void ts_lex_fill_unicode_class_bounds(
  const TSLexTableView *v,
  uint32_t *cls_lo,
  uint32_t *cls_hi
) {
  for (uint32_t c = 0; c < v->class_count; c++) {
    cls_lo[c] = 0x110000u;
    cls_hi[c] = 0u;
  }
  uint32_t start = 0x80u;
  for (uint32_t i = 0; i < v->unicode_range_count; i++) {
    uint32_t end = ts_lex_read_u32(&v->unicode_ends[i * 2u]);
    uint16_t cls = v->unicode_classes[i];
    if (start < cls_lo[cls]) cls_lo[cls] = start;
    cls_hi[cls] = end;
    start = end + 1u;
  }
}

static void ts_lex_collect_template_meta(
  const TSLexTableView *v,
  uint32_t *tmpl_base,
  uint16_t *tmpl_def
) {
  for (uint32_t s = 0; s < v->state_count; s++) {
    TSLexStateMeta sm = ts_lex_state_meta(v, (uint16_t)s);
    if (sm.tmpl == 0) continue;
    if (tmpl_base[sm.tmpl] == UINT32_MAX) {
      tmpl_base[sm.tmpl] = sm.base;
      tmpl_def[sm.tmpl] = sm.def;
    } else if (tmpl_base[sm.tmpl] != sm.base || tmpl_def[sm.tmpl] != sm.def) {
      tmpl_base[sm.tmpl] = UINT32_MAX - 1u;
    }
  }
}

static void ts_lex_accumulate_comb_exceptions(
  const TSLexTableView *v,
  const LexUnicodeExcScratch *s
) {
  for (uint32_t i = 0; i < v->comb_len; i++) {
    uint16_t t = v->comb[i * 2u];
    if (t == 0 || t >= v->template_count) continue;
    uint32_t base = s->tmpl_base[t];
    if (base == UINT32_MAX - 1u) {
      ts_lex_widen_bounds(&s->min_lo[t], &s->max_hi[t], 0x80u, 0x10ffffu);
      continue;
    }
    if (base == UINT32_MAX || i < base) continue;
    uint32_t cls = i - base;
    if (cls < v->class_count && v->comb[i * 2u + 1u] != s->tmpl_def[t]) {
      ts_lex_widen_bounds(&s->min_lo[t], &s->max_hi[t], s->cls_lo[cls], s->cls_hi[cls]);
    }
  }
}

static inline void ts_lex_widen_bounds(uint32_t *min_lo, uint32_t *max_hi, uint32_t lo, uint32_t hi) {
  if (lo > hi) return;
  if (lo < *min_lo) *min_lo = lo;
  if (hi > *max_hi) *max_hi = hi;
}

static inline void ts_lex_apply_accept(
  Lexer *self,
  const TSLexTableView *v,
  uint16_t acc,
  LexRunState *run
) {
  if (acc == 0) return;
  if (acc < TS_LEX_EXTENDED_FLAG) {
    uint32_t start_byte = (acc & TS_LEX_SIMPLE_ACCEPT_TAG0_FLAG)
      ? run->tags[0]._opaque[0]
      : self->token_start_position.bytes;
    bool has_err = false;
    if (!ts_lex_check_accept_decode_error(self, start_byte, &has_err)) return;
    if (acc & TS_LEX_SIMPLE_ACCEPT_TAG0_FLAG) {
      self->data.mark_start(&self->data, &run->tags[0]);
    }
    run->result = true;
    self->data.result_symbol = (TSSymbol)((acc & TS_LEX_SIMPLE_ACCEPT_MASK) - 1u);
    self->data.mark_end(&self->data);
    self->token_has_decode_error = has_err;
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
    ts_lex_restore_fallback_accept(self, run);
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
  uint32_t start_byte = tag != UINT16_MAX ? run->tags[tag]._opaque[0] : self->token_start_position.bytes;
  bool has_err = false;
  if (!allowed || !ts_lex_check_accept_decode_error(self, start_byte, &has_err)) return;
  if (tag != UINT16_MAX) self->data.mark_start(&self->data, &run->tags[tag]);
  run->result = true;
  self->data.result_symbol = sym;
  self->data.mark_end(&self->data);
  self->token_has_decode_error = has_err;
}

static inline void ts_lex_restore_fallback_accept(Lexer *self, LexRunState *run) {
  run->result = run->fb.valid;
  if (!run->result) {
    self->token_has_decode_error = false;
    return;
  }
  self->data.result_symbol = run->fb.symbol;
  self->data.mark_start(&self->data, &run->fb.start);
  self->data.mark_end_at(&self->data, &run->fb.end);
  self->token_has_decode_error = run->fb.has_decode_error;
}

static const uint16_t *ts_lex_apply_fallback_accept(
  Lexer *self,
  const uint16_t *p,
  LexRunState *run
) {
  TSSymbol sym = *p++;
  int32_t prec = (int32_t)ts_lex_read_u32(p);
  uint16_t tag = p[2];
  uint32_t start_byte = tag != UINT16_MAX ? run->tags[tag]._opaque[0] : self->token_start_position.bytes;
  bool has_err = false;
  if ((!run->fb.valid || prec >= run->fb.precedence) &&
      ts_lex_check_accept_decode_error(self, start_byte, &has_err)) {
    run->fb.valid = true;
    run->fb.has_decode_error = has_err;
    run->fb.precedence = prec;
    run->fb.symbol = sym;
    if (tag != UINT16_MAX) {
      run->fb.start = run->tags[tag];
    } else {
      run->fb.start = (TSLexerCheckpoint){{
        self->token_start_position.bytes,
        self->token_start_position.extent.row,
        self->token_start_position.extent.column,
      }};
    }
    self->data.mark_end(&self->data);
    self->data.get_mark_end_checkpoint(&self->data, &run->fb.end);
  }
  return p + 3;
}

static inline bool ts_lex_check_accept_decode_error(
  const Lexer *self,
  uint32_t accept_start_byte,
  bool *out_has_decode_error
) {
  if (!self->has_decode_error) {
    *out_has_decode_error = false;
    return true;
  }
  if (self->first_decode_error_byte < accept_start_byte) return false;
  *out_has_decode_error = true;
  return true;
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
