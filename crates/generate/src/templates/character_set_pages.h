static inline bool ts_lex_pagesWIDTH_contains(
  const uint32_t *ascii,
  const uintWIDTH_t *page_ids,
  const uint32_t (*pages)[8],
  uint32_t first,
  uint32_t count,
  int32_t lookahead
) {
  if ((uint32_t)lookahead < 128) {
    return (ascii[lookahead / 32] >> (lookahead % 32)) & 1;
  }
  uint32_t page = ((uint32_t)lookahead / 256) - first;
  return page < count &&
    ((pages[page_ids[page]][((uint32_t)lookahead % 256) / 32] >>
      ((uint32_t)lookahead % 32)) & 1);
}
