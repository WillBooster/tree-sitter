#ifndef TREE_SITTER_WEB_MEMORY_LIMIT_H_
#define TREE_SITTER_WEB_MEMORY_LIMIT_H_

#include <setjmp.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef union WebAllocation WebAllocation;

typedef struct {
  jmp_buf jump;
  WebAllocation *allocations;
} WebParseBudget;

union WebAllocation {
  struct {
    WebAllocation *previous;
    WebAllocation *next;
    WebParseBudget *budget;
  } data;
  max_align_t alignment;
};

static WebParseBudget *current_parse_budget;

static void web_allocation_failed(void) {
  if (current_parse_budget) longjmp(current_parse_budget->jump, 1);
  abort();
}

static void *web_malloc(size_t size) {
  if (size > SIZE_MAX - sizeof(WebAllocation)) web_allocation_failed();
  WebAllocation *allocation = malloc(sizeof(WebAllocation) + size);
  if (!allocation) web_allocation_failed();
  allocation->data.previous = NULL;
  allocation->data.budget = current_parse_budget;
  allocation->data.next = current_parse_budget ? current_parse_budget->allocations : NULL;
  if (allocation->data.next) allocation->data.next->data.previous = allocation;
  if (current_parse_budget) current_parse_budget->allocations = allocation;
  return allocation + 1;
}

static void *web_calloc(size_t count, size_t size) {
  if (size && count > SIZE_MAX / size) web_allocation_failed();
  size_t bytes = count * size;
  void *result = web_malloc(bytes);
  memset(result, 0, bytes);
  return result;
}

static void web_free(void *pointer) {
  if (!pointer) return;
  WebAllocation *allocation = (WebAllocation *)pointer - 1;
  WebParseBudget *budget = allocation->data.budget;
  if (budget) {
    if (allocation->data.previous) allocation->data.previous->data.next = allocation->data.next;
    else budget->allocations = allocation->data.next;
    if (allocation->data.next) allocation->data.next->data.previous = allocation->data.previous;
  }
  free(allocation);
}

static void *web_realloc(void *pointer, size_t size) {
  if (!pointer) return web_malloc(size);
  if (!size) {
    web_free(pointer);
    return NULL;
  }
  if (size > SIZE_MAX - sizeof(WebAllocation)) web_allocation_failed();
  WebAllocation *allocation = (WebAllocation *)pointer - 1;
  WebParseBudget *budget = allocation->data.budget;
  WebAllocation *previous = allocation->data.previous;
  WebAllocation *next = allocation->data.next;
  allocation = realloc(allocation, sizeof(WebAllocation) + size);
  if (!allocation) web_allocation_failed();
  if (budget) {
    if (previous) previous->data.next = allocation;
    else budget->allocations = allocation;
    if (next) next->data.previous = allocation;
  }
  return allocation + 1;
}

static void web_budget_discard(WebParseBudget *budget) {
  // A failed allocation can unwind from a half-updated parse; discard its allocations without walking that stack.
  current_parse_budget = NULL;
  for (WebAllocation *allocation = budget->allocations; allocation;) {
    WebAllocation *next = allocation->data.next;
    free(allocation);
    allocation = next;
  }
  free(budget);
}

static void web_budget_finish(WebParseBudget *budget) {
  current_parse_budget = NULL;
  for (WebAllocation *allocation = budget->allocations; allocation; allocation = allocation->data.next) {
    allocation->data.budget = NULL;
  }
  free(budget);
}

#endif
