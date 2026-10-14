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
