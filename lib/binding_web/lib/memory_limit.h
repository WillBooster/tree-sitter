#ifndef TREE_SITTER_WEB_MEMORY_LIMIT_H_
#define TREE_SITTER_WEB_MEMORY_LIMIT_H_

#include <setjmp.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

typedef struct TSParser TSParser;

typedef struct {
  jmp_buf jump;
  void **entries;
  size_t count;
  size_t capacity;
  TSParser *parser;
  void *ranges;
} WebParseBudget;

static WebParseBudget *current_parse_budget;

static void web_allocation_failed(void) {
  if (current_parse_budget) longjmp(current_parse_budget->jump, 1);
  abort();
}

static inline size_t web_hash_ptr(const void *pointer) {
  size_t value = (size_t)(uintptr_t)pointer;
  value ^= value >> 16;
  value *= 0x85ebca6bu;
  value ^= value >> 13;
  return value;
}

static inline void web_budget_insert_reserved(WebParseBudget *budget, void *pointer) {
  size_t mask = budget->capacity - 1;
  size_t slot = web_hash_ptr(pointer) & mask;
  while (budget->entries[slot]) {
    slot = (slot + 1) & mask;
  }
  budget->entries[slot] = pointer;
  budget->count++;
}

static void web_budget_reserve(WebParseBudget *budget) {
  if ((budget->count + 1) * 4 < budget->capacity * 3) return;
  size_t new_capacity = budget->capacity ? budget->capacity * 2 : 64;
  if (new_capacity < budget->capacity || new_capacity > SIZE_MAX / sizeof(void *)) {
    web_allocation_failed();
  }
  void **new_entries = calloc(new_capacity, sizeof(void *));
  if (!new_entries) web_allocation_failed();
  void **old_entries = budget->entries;
  size_t old_capacity = budget->capacity;
  budget->entries = new_entries;
  budget->capacity = new_capacity;
  budget->count = 0;
  for (size_t i = 0; i < old_capacity; i++) {
    if (old_entries[i]) web_budget_insert_reserved(budget, old_entries[i]);
  }
  free(old_entries);
}

static bool web_budget_remove(WebParseBudget *budget, const void *pointer) {
  if (!budget->capacity) return false;
  size_t mask = budget->capacity - 1;
  size_t slot = web_hash_ptr(pointer) & mask;
  for (;;) {
    void *entry = budget->entries[slot];
    if (!entry) return false;
    if (entry == pointer) break;
    slot = (slot + 1) & mask;
  }
  size_t empty = slot;
  for (size_t cursor = (slot + 1) & mask; budget->entries[cursor]; cursor = (cursor + 1) & mask) {
    size_t home = web_hash_ptr(budget->entries[cursor]) & mask;
    if (((cursor - home) & mask) >= ((cursor - empty) & mask)) {
      budget->entries[empty] = budget->entries[cursor];
      empty = cursor;
    }
  }
  budget->entries[empty] = NULL;
  budget->count--;
  return true;
}

static void *web_malloc(size_t size) {
  size_t actual_size = size ? size : 1;
  if (!current_parse_budget) {
    void *result = malloc(actual_size);
    if (!result) abort();
    return result;
  }
  web_budget_reserve(current_parse_budget);
  void *result = malloc(actual_size);
  if (!result) web_allocation_failed();
  web_budget_insert_reserved(current_parse_budget, result);
  return result;
}

static void *web_calloc(size_t count, size_t size) {
  if (size && count > SIZE_MAX / size) web_allocation_failed();
  size_t actual_count = count ? count : 1;
  size_t actual_size = size ? size : 1;
  if (!current_parse_budget) {
    void *result = calloc(actual_count, actual_size);
    if (!result) abort();
    return result;
  }
  web_budget_reserve(current_parse_budget);
  void *result = calloc(actual_count, actual_size);
  if (!result) web_allocation_failed();
  web_budget_insert_reserved(current_parse_budget, result);
  return result;
}

static void web_free(void *pointer) {
  if (!pointer) return;
  if (current_parse_budget) web_budget_remove(current_parse_budget, pointer);
  free(pointer);
}

static void *web_realloc(void *pointer, size_t size) {
  if (!pointer) return web_malloc(size);
  if (!size) {
    web_free(pointer);
    return NULL;
  }
  if (!current_parse_budget) {
    void *result = realloc(pointer, size);
    if (!result) abort();
    return result;
  }
  void *result = realloc(pointer, size);
  if (!result) web_allocation_failed();
  if (result != pointer && web_budget_remove(current_parse_budget, pointer)) {
    web_budget_insert_reserved(current_parse_budget, result);
  }
  return result;
}

static WebParseBudget *web_budget_new(void *ranges) {
  WebParseBudget *budget = calloc(1, sizeof(WebParseBudget));
  if (!budget) {
    free(ranges);
    return NULL;
  }
  budget->ranges = ranges;
  return budget;
}

static void web_budget_discard(WebParseBudget *budget) {
  // A failed allocation can unwind from a half-updated parse; discard its allocations without walking that stack.
  current_parse_budget = NULL;
  free(budget->ranges);
  for (size_t i = 0; i < budget->capacity; i++) {
    if (budget->entries[i]) free(budget->entries[i]);
  }
  free(budget->entries);
  free(budget);
}

static void web_budget_finish(WebParseBudget *budget) {
  current_parse_budget = NULL;
  free(budget->entries);
  free(budget);
}

#endif
