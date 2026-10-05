#define TS_LEX_REPEAT(skip_value, label)             \
  {                                                \
    lexer->advance(lexer, skip_value);               \
    lookahead = lexer->lookahead;                    \
    eof = lookahead == 0 && lexer->eof(lexer);        \
    goto label;                                     \
  }

