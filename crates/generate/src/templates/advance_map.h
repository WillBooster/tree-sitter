#define TS_LEX_ADVANCE_MAP_SORTED(map)                                      \
  {                                                                 \
    uint32_t low = 0;                                                \
    uint32_t high = sizeof(map) / sizeof(map[0]) / 2;                 \
    while (low < high) {                                             \
      uint32_t mid = low + (high - low) / 2;                          \
      if (map[mid * 2] < lookahead) {                                \
        low = mid + 1;                                              \
      } else if (map[mid * 2] > lookahead) {                          \
        high = mid;                                                 \
      } else {                                                      \
        state = map[mid * 2 + 1];                                    \
        goto next_state;                                            \
      }                                                             \
    }                                                               \
  }

#define TS_LEX_ADVANCE_MAP_DENSE(first, map)                                \
  {                                                                 \
    uint32_t index = (uint32_t)lookahead - (first);                   \
    if (index < sizeof(map) / sizeof(map[0]) && map[index] != UINT16_MAX) { \
      state = map[index];                                            \
      goto next_state;                                              \
    }                                                               \
  }

