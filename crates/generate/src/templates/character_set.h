static inline bool ts_lex_set_contains_with_ascii(
  const uint32_t *ascii,
  const TSCharacterRange *ranges,
  uint32_t len,
  int32_t lookahead
) {
  if ((uint32_t)lookahead < 128) {
    return (ascii[lookahead / 32] >> (lookahead % 32)) & 1;
  }
  return len && set_contains(ranges, len, lookahead);
}
#define TS_LEX_PAGES_CONTAINS(width)                                      \
  static inline bool ts_lex_pages##width##_contains(                      \
    const uint32_t *ascii, const uint##width##_t *page_ids,                \
    const uint32_t (*pages)[8], uint32_t first, uint32_t count,             \
    int32_t lookahead                                                    \
  ) {                                                                    \
    if ((uint32_t)lookahead < 128) {                                      \
      return (ascii[lookahead / 32] >> (lookahead % 32)) & 1;               \
    }                                                                    \
    uint32_t page = ((uint32_t)lookahead / 256) - first;                   \
    return page < count &&                                               \
      ((pages[page_ids[page]][((uint32_t)lookahead % 256) / 32] >>          \
        ((uint32_t)lookahead % 32)) & 1);                                 \
  }

TS_LEX_PAGES_CONTAINS(8)
TS_LEX_PAGES_CONTAINS(16)
#undef TS_LEX_PAGES_CONTAINS
