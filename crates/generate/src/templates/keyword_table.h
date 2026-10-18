
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
  TSSymbol root_accept = ts_keyword_states[0].accept;
  uint32_t prefix = root_accept ? TS_KEYWORD_PREFIX : 0;
  if (!length) return prefix | root_accept;
  uint16_t first = ts_keyword_first_byte[(uint8_t)text[0]];
  if (first == UINT16_MAX) return prefix;
  uint16_t state = first & TS_KEYWORD_STATE_MASK;
  for (uint32_t i = 1; i < length; i++) {
    if (ts_keyword_states[state].accept) prefix = TS_KEYWORD_PREFIX;
    uint8_t byte = (uint8_t)text[i];
    if (byte >= 128) return prefix;
    uint16_t next = ts_keyword_next(state, byte);
    if (next == UINT16_MAX) return prefix;
    state = next & TS_KEYWORD_STATE_MASK;
  }
  TSSymbol accept = ts_keyword_states[state].accept;
  return prefix | (accept ? TS_KEYWORD_PREFIX | accept : 0);
}
