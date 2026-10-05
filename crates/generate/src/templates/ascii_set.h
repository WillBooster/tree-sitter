static inline bool ts_lex_ascii_contains(const uint32_t *bits, int32_t lookahead) {
  return (uint32_t)lookahead < 128 && ((bits[lookahead / 32] >> (lookahead % 32)) & 1);
}

