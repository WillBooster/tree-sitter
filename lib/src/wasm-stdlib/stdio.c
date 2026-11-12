// Scanner-oriented stdio compatibility functions. Formatting writes to memory,
// while stream operations intentionally perform no I/O.

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct {
  bool left_justify;   // -
  bool zero_pad;       // 0
  bool show_sign;      // +
  bool space_prefix;   // ' '
  bool alternate_form; // #
} format_flags_t;

static const char* parse_format_spec(
  const char *format,
  int *width,
  int *precision,
  format_flags_t *flags
) {
  *width = 0;
  *precision = -1;
  flags->left_justify = false;
  flags->zero_pad = false;
  flags->show_sign = false;
  flags->space_prefix = false;
  flags->alternate_form = false;

  const char *p = format;

  // Parse flags
  while (*p == '-' || *p == '+' || *p == ' ' || *p == '#' || *p == '0') {
    switch (*p) {
      case '-': flags->left_justify = true; break;
      case '0': flags->zero_pad = true; break;
      case '+': flags->show_sign = true; break;
      case ' ': flags->space_prefix = true; break;
      case '#': flags->alternate_form = true; break;
    }
    p++;
  }

  // width
  while (*p >= '0' && *p <= '9') {
    *width = (*width * 10) + (*p - '0');
    p++;
  }

  // precision
  if (*p == '.') {
    p++;
    *precision = 0;
    while (*p >= '0' && *p <= '9') {
      *precision = (*precision * 10) + (*p - '0');
      p++;
    }
  }

  return p;
}

static int int_to_str(
  long long value,
  char *buffer,
  int base,
  bool is_signed,
  bool uppercase
) {
  if (base < 2 || base > 16) return 0;

  const char *digits = uppercase ? "0123456789ABCDEF" : "0123456789abcdef";
  char temp[32];
  int i = 0, len = 0;
  bool is_negative = false;

  if (value == 0) {
    buffer[0] = '0';
    buffer[1] = '\0';
    return 1;
  }

  if (is_signed && value < 0 && base == 10) {
    is_negative = true;
  }

  unsigned long long uval = is_negative
    ? 0ULL - (unsigned long long)value
    : (unsigned long long)value;
  while (uval > 0) {
    temp[i++] = digits[uval % base];
    uval /= base;
  }

  if (is_negative) {
    buffer[len++] = '-';
  }

  while (i > 0) {
    buffer[len++] = temp[--i];
  }

  buffer[len] = '\0';
  return len;
}

static int ptr_to_str(void *ptr, char *buffer) {
  buffer[0] = '0';
  buffer[1] = 'x';
  int len = int_to_str((uintptr_t)ptr, buffer + 2, 16, 0, 0);
  return 2 + len;
}

typedef struct {
  char *data;
  size_t size;
  size_t pos;
} output_buffer_t;

static void append_char(output_buffer_t *out, char ch) {
  if (out->size > 1 && out->pos < out->size - 1) {
    out->data[out->pos++] = ch;
  }
}

static void append_repeated(output_buffer_t *out, char ch, int count) {
  for (int i = 0; i < count; i++) {
    append_char(out, ch);
  }
}

static void write_formatted_to_buffer(
  output_buffer_t *out,
  const char *str,
  int len,
  int width,
  const format_flags_t *flags
) {
  int pad_len = (width > len) ? (width - len) : 0;
  if (!flags->left_justify) {
    append_repeated(out, flags->zero_pad ? '0' : ' ', pad_len);
  }
  for (int i = 0; i < len; i++) {
    append_char(out, str[i]);
  }
  if (flags->left_justify) {
    append_repeated(out, ' ', pad_len);
  }
}

static long long read_signed_arg(va_list *args, int length_mod) {
  if (length_mod >= 2) return va_arg(*args, long long);
  if (length_mod == 1) return va_arg(*args, long);
  return va_arg(*args, int);
}

static unsigned long long read_unsigned_arg(va_list *args, int length_mod) {
  if (length_mod >= 2) return va_arg(*args, unsigned long long);
  if (length_mod == 1) return va_arg(*args, unsigned long);
  return va_arg(*args, unsigned int);
}

static int format_string_arg(const char *str, int precision, const char **output_str) {
  if (!str) str = "(null)";
  *output_str = str;
  int str_len = (int)strlen(str);
  return (precision >= 0 && str_len > precision) ? precision : str_len;
}

static int format_char_or_unknown(char spec, va_list *args, char *temp_buf) {
  if (spec == 'c') {
    temp_buf[0] = (char)va_arg(*args, int);
    temp_buf[1] = '\0';
    return 1;
  }
  temp_buf[0] = '%';
  temp_buf[1] = spec;
  temp_buf[2] = '\0';
  return spec ? 2 : 1;
}

static int format_specifier(
  const char **fmt_ptr,
  va_list *args,
  int precision,
  char *temp_buf,
  const char **output_str
) {
  const char *p = *fmt_ptr;
  int length_mod = 0;
  for (; *p == 'l'; p++) length_mod++;
  if (*p == 'z' && *(p + 1) == 'u') {
    *fmt_ptr = p + 1;
    *output_str = temp_buf;
    return int_to_str((long long)va_arg(*args, size_t), temp_buf, 10, false, false);
  }
  *fmt_ptr = p;
  *output_str = temp_buf;
  switch (*p) {
    case 's': return format_string_arg(va_arg(*args, const char *), precision, output_str);
    case 'd':
    case 'i': return int_to_str(read_signed_arg(args, length_mod), temp_buf, 10, true, false);
    case 'u':
    case 'x':
    case 'X':
      return int_to_str(
        (long long)read_unsigned_arg(args, length_mod), temp_buf, *p == 'u' ? 10 : 16, false, *p == 'X'
      );
    case 'p': return ptr_to_str(va_arg(*args, void *), temp_buf);
    default: return format_char_or_unknown(*p, args, temp_buf);
  }
}

static bool format_percent(const char **fmt_ptr, va_list *args, output_buffer_t *out, int *total_chars) {
  const char *p = *fmt_ptr;
  if (*p == '%') {
    append_char(out, '%');
    (*total_chars)++;
    return true;
  }

  int width, precision;
  format_flags_t flags;
  p = parse_format_spec(p, &width, &precision, &flags);
  if (!*p) return false;

  char temp_buf[64];
  const char *output_str = temp_buf;
  int str_len = format_specifier(&p, args, precision, temp_buf, &output_str);
  if (!*p) return false;

  *fmt_ptr = p;
  *total_chars += (width > str_len) ? width : str_len;
  write_formatted_to_buffer(out, output_str, str_len, width, &flags);
  return true;
}

static int vsnprintf_impl(char *buffer, size_t buffsz, const char *format, va_list args) {
  if (!format || (buffsz > 0 && !buffer)) return -1;

  va_list args_copy;
  va_copy(args_copy, args);

  output_buffer_t out = {buffer, buffsz, 0};
  int total_chars = 0;
  for (const char *p = format; *p; p++) {
    if (*p != '%') {
      append_char(&out, *p);
      total_chars++;
      continue;
    }
    p++;
    if (!format_percent(&p, &args_copy, &out, &total_chars)) break;
  }

  va_end(args_copy);
  if (buffsz > 0) {
    size_t nul_pos = out.pos < buffsz ? out.pos : buffsz - 1;
    buffer[nul_pos] = '\0';
  }

  return total_chars;
}

int snprintf(char *restrict buffer, size_t buffsz, const char *restrict format, ...) {
  va_list args;
  va_start(args, format);
  int result = vsnprintf_impl(buffer, buffsz, format, args);
  va_end(args);

  return result;
}

int vsnprintf(char *restrict buffer, size_t buffsz, const char *restrict format, va_list vlist) {
  return vsnprintf_impl(buffer, buffsz, format, vlist);
}

#ifndef __EMSCRIPTEN__
int fclose(FILE *stream) {
  return 0;
}

FILE* fdopen(int fd, const char *mode) {
  return 0;
}

int fputc(int c, FILE *stream) {
  return c;
}

int fputs(const char *restrict str, FILE *restrict stream) {
  return 0;
}

size_t fwrite(const void *restrict buffer, size_t size, size_t nmemb, FILE *restrict stream) {
  return size * nmemb;
}

int fprintf(FILE *restrict stream, const char *restrict format, ...) {
  (void)stream;
  (void)format;
  return 0;
}
#endif
