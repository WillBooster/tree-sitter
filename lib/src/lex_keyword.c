#include "./lex_table.h"
#include "./unicode.h"

static inline bool ts_lex_can_lookup_keyword(const uint16_t *table, TSInputEncoding encoding);
static int ts_lex_lookup_keyword_ascii_prefix(
  const TSLexTableView *v, const uint8_t *bytes, uint32_t byte_len, uint32_t stride,
  uint32_t *offset, uint16_t *state, bool *is_keyword
);
static TSLexKeywordResult ts_lex_lookup_keyword_from(
  const TSLexTableView *v, const uint8_t *bytes, uint32_t byte_len, TSInputEncoding encoding,
  uint16_t state, bool is_keyword
);
static inline bool ts_lex_keyword_step(const TSLexTableView *v, uint16_t act, uint16_t *state);
static inline uint32_t ts_lex_decode_keyword_char(
  const uint8_t *bytes, uint32_t remaining, TSInputEncoding encoding, int32_t *cp
);

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
  if (!v->wide_base && (encoding == TSInputEncodingUTF8 || encoding == TSInputEncodingUTF16LE)) {
    uint32_t stride = encoding == TSInputEncodingUTF8 ? 1u : 2u;
    int status = ts_lex_lookup_keyword_ascii_prefix(
      v, bytes, byte_len, stride, &offset, &state, &is_keyword
    );
    if (status < 0) return (TSLexKeywordResult){0, false, false};
    if (status == 0) return (TSLexKeywordResult){0, true, is_keyword};
  }
  return ts_lex_lookup_keyword_from(v, bytes + offset, byte_len - offset, encoding, state, is_keyword);
}

static inline bool ts_lex_can_lookup_keyword(const uint16_t *table, TSInputEncoding encoding) {
  return table &&
    (table[6] & TS_LEX_DIRECT_KEYWORD_FLAG) &&
    (encoding == TSInputEncodingUTF8 ||
     encoding == TSInputEncodingUTF16LE ||
     encoding == TSInputEncodingUTF16BE);
}

// code-gauge-allow functionParameterCount 7 -- carries in-out offset/state/is_keyword cursor to resume non-ASCII tail
static int ts_lex_lookup_keyword_ascii_prefix(
  const TSLexTableView *v,
  const uint8_t *bytes,
  uint32_t byte_len,
  uint32_t stride,
  uint32_t *offset,
  uint16_t *state,
  bool *is_keyword
) {
  uint32_t idx = *offset;
  uint16_t cur_state = *state;
  bool kw = *is_keyword;
  while (idx + stride <= byte_len) {
    uint16_t u = stride == 1u
      ? bytes[idx]
      : ((uint16_t)bytes[idx] | ((uint16_t)bytes[idx + 1u] << 8));
    if (u >= 0x80u) break;
    const uint16_t *sm = &v->state_meta[(uint32_t)cur_state * 4u];
    if (sm[3] != 0) kw = true;
    uint16_t tmpl = sm[1];
    if (tmpl == 0) {
      *is_keyword = kw;
      return 0;
    }
    const uint16_t *ce = &v->comb[((uint32_t)sm[0] + v->ascii_class[u]) * 2u];
    uint16_t act = ce[0] == tmpl ? ce[1] : sm[2];
    if (act == 0) {
      *is_keyword = kw;
      return 0;
    }
    if (!ts_lex_keyword_step(v, act, &cur_state)) return -1;
    idx += stride;
  }
  *offset = idx;
  *state = cur_state;
  *is_keyword = kw;
  return 1;
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
