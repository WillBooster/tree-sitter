
static inline uint16_t ts_keyword_next(uint16_t state, uint32_t character) {
  const TSKeywordState *row = &ts_keyword_states[state];
  uint32_t low = row->index, high = ts_keyword_states[state + 1].index;
  while (low < high) {
    uint32_t mid = low + (high - low) / 2;
    TSKeywordTransition transition = ts_keyword_transitions[mid];
    if (character < transition.first) high = mid;
    else if (character > transition.last) low = mid + 1;
    else return transition.state;
  }
  return UINT16_MAX;
}

static bool ts_lex_keywords(TSLexer *lexer, TSStateId state) {
  bool result = false;
  for (;;) {
    TSSymbol accept = ts_keyword_states[state].accept;
    if (accept) {
      lexer->result_symbol = accept;
      lexer->mark_end(lexer);
      result = true;
    }
    uint16_t next = ts_keyword_next(state, (uint32_t)lexer->lookahead);
    if (next == UINT16_MAX) return result;
    state = next & TS_KEYWORD_STATE_MASK;
    lexer->advance(lexer, (next & TS_KEYWORD_SKIP) != 0);
  }
}

static uint32_t ts_keyword_lookup(const char *text, uint32_t length) {
  uint16_t state = 0;
  uint32_t prefix = 0;
  for (uint32_t i = 0; i < length; i++) {
    if (ts_keyword_states[state].accept) prefix = TS_KEYWORD_PREFIX;
    uint16_t next = ts_keyword_next(state, (uint8_t)text[i]);
    if (next == UINT16_MAX) return prefix;
    state = next & TS_KEYWORD_STATE_MASK;
  }
  TSSymbol accept = ts_keyword_states[state].accept;
  return prefix | (accept ? TS_KEYWORD_PREFIX | accept : 0);
}
