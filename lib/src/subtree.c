// code-gauge-allow-file fileNcss 570 -- subtree construction, summaries, and printing share private helpers here
#include <ctype.h>
#include <stdint.h>
#include <stdbool.h>
#include <string.h>
#include <stdio.h>
#include "./alloc.h"
#include "./array.h"
#include "./atomic.h"
#include "./subtree.h"
#include "./length.h"
#include "./language.h"
#include "./error_costs.h"
#include "./ts_assert.h"
#include <stddef.h>

#define TS_MAX_INLINE_TREE_LENGTH UINT8_MAX
#define TS_MAX_INLINE_TREE_SYMBOL 0x1FFu
#define TS_MAX_TREE_POOL_SIZE 128

_Static_assert(sizeof(SubtreeInlineData) == 8, "an inline subtree must fit in 8 bytes");
_Static_assert(sizeof(SubtreeHeapData) == 64, "a heap subtree must fit in 64 bytes");

// ExternalScannerState

static inline bool ts_external_scanner_state_is_long(const ExternalScannerState *self) {
  return self->length > sizeof(self->data);
}

static inline char *ts_external_scanner_state_long_data(const ExternalScannerState *self) {
  char *result;
  memcpy(&result, self->data, sizeof(result));
  return result;
}

static inline void ts_external_scanner_state_set_long_data(ExternalScannerState *self, const char *data) {
  char *copy = ts_malloc(self->length);
  memcpy(copy, data, self->length);
  memcpy(self->data, &copy, sizeof(copy));
}

void ts_external_scanner_state_init(ExternalScannerState *self, const char *data, unsigned length) {
  self->length = length;
  if (ts_external_scanner_state_is_long(self)) {
    ts_external_scanner_state_set_long_data(self, data);
  } else {
    memcpy(self->data, data, length);
  }
}

ExternalScannerState ts_external_scanner_state_copy(const ExternalScannerState *self) {
  ExternalScannerState result = *self;
  if (ts_external_scanner_state_is_long(self)) {
    ts_external_scanner_state_set_long_data(&result, ts_external_scanner_state_long_data(self));
  }
  return result;
}

void ts_external_scanner_state_delete(ExternalScannerState *self) {
  if (ts_external_scanner_state_is_long(self)) {
    ts_free(ts_external_scanner_state_long_data(self));
  }
}

const char *ts_external_scanner_state_data(const ExternalScannerState *self) {
  return ts_external_scanner_state_is_long(self) ? ts_external_scanner_state_long_data(self) : self->data;
}

bool ts_external_scanner_state_eq(const ExternalScannerState *self, const char *buffer, unsigned length) {
  return
    self->length == length &&
    memcmp(ts_external_scanner_state_data(self), buffer, length) == 0;
}

// SubtreeArray

void ts_subtree_array_copy(SubtreeArray self, SubtreeArray *dest) {
  dest->size = self.size;
  dest->capacity = self.capacity;
  dest->contents = self.contents;
  if (self.capacity > 0) {
    dest->contents = ts_calloc(self.capacity, sizeof(Subtree));
    memcpy(dest->contents, self.contents, self.size * sizeof(Subtree));
    for (uint32_t i = 0; i < self.size; i++) {
      ts_subtree_retain(*array_get(dest, i));
    }
  }
}

void ts_subtree_array_clear(SubtreePool *pool, SubtreeArray *self) {
  for (uint32_t i = 0; i < self->size; i++) {
    ts_subtree_release(pool, *array_get(self, i));
  }
  array_clear(self);
}

void ts_subtree_array_delete(SubtreePool *pool, SubtreeArray *self) {
  ts_subtree_array_clear(pool, self);
  array_delete(self);
}

void ts_subtree_array_remove_trailing_extras(
  SubtreeArray *self,
  SubtreeArray *destination
) {
  array_clear(destination);
  while (self->size > 0) {
    Subtree last = *array_get(self, self->size - 1);
    if (ts_subtree_extra(last)) {
      self->size--;
      array_push(destination, last);
    } else {
      break;
    }
  }
  ts_subtree_array_reverse(destination);
}

void ts_subtree_array_reverse(SubtreeArray *self) {
  for (uint32_t i = 0, limit = self->size / 2; i < limit; i++) {
    size_t reverse_index = self->size - 1 - i;
    Subtree swap = *array_get(self, i);
    *array_get(self, i) = *array_get(self, reverse_index);
    *array_get(self, reverse_index) = swap;
  }
}

// SubtreePool

SubtreePool ts_subtree_pool_new(uint32_t capacity) {
  SubtreePool self = {array_new(), array_new()};
  array_reserve(&self.free_trees, capacity);
  return self;
}

void ts_subtree_pool_delete(SubtreePool *self) {
  if (self->free_trees.contents) {
    for (unsigned i = 0; i < self->free_trees.size; i++) {
      ts_free(array_get(&self->free_trees, i)->ptr);
    }
    array_delete(&self->free_trees);
  }
  if (self->tree_stack.contents) array_delete(&self->tree_stack);
}

static SubtreeHeapData *ts_subtree_pool_allocate(SubtreePool *self) {
  if (self->free_trees.size > 0) {
    return array_pop(&self->free_trees).ptr;
  } else {
    return ts_malloc(sizeof(SubtreeHeapData));
  }
}

static void ts_subtree_pool_free(SubtreePool *self, SubtreeHeapData *tree) {
  if (self->free_trees.capacity > 0 && self->free_trees.size + 1 <= TS_MAX_TREE_POOL_SIZE) {
    array_push(&self->free_trees, (MutableSubtree) {.ptr = tree});
  } else {
    ts_free(tree);
  }
}

// Subtree

static inline bool ts_subtree_can_inline(Length padding, Length size) {
  return
    padding.bytes < TS_MAX_INLINE_TREE_LENGTH &&
    padding.extent.row < TS_MAX_INLINE_TREE_LENGTH &&
    padding.extent.column < TS_MAX_INLINE_TREE_LENGTH &&
    size.bytes < TS_MAX_INLINE_TREE_LENGTH &&
    size.extent.row == 0 &&
    size.extent.column < TS_MAX_INLINE_TREE_LENGTH;
}

// code-gauge-allow functionParameterCount 9 -- the lexer passes every property of the token it produced
Subtree ts_subtree_new_leaf(
  SubtreePool *pool, TSSymbol symbol, Length padding, Length size,
  TSStateId parse_state, bool has_external_tokens,
  bool is_keyword, bool is_keyword_exact, const TSLanguage *language
) {
  TSSymbolMetadata metadata = ts_language_symbol_metadata(language, symbol);
  bool extra = symbol == ts_builtin_sym_end;

  bool is_inline = (
    symbol <= TS_MAX_INLINE_TREE_SYMBOL &&
    !has_external_tokens &&
    ts_subtree_can_inline(padding, size)
  );

  if (is_inline) {
    return (Subtree) {{
      .parse_state = parse_state,
      .symbol = (uint8_t)symbol,
      .symbol_msb = (uint8_t)(symbol >> 8),
      .padding_bytes = padding.bytes,
      .padding_rows = padding.extent.row,
      .padding_columns = padding.extent.column,
      .size_bytes = size.bytes,
      .visible = metadata.visible,
      .named = metadata.named,
      .extra = extra,
      .is_missing = false,
      .is_keyword = is_keyword,
      .is_keyword_exact = is_keyword_exact,
      .is_inline = true,
    }};
  } else {
    SubtreeHeapData *data = ts_subtree_pool_allocate(pool);
    *data = (SubtreeHeapData) {
      .ref_count = 1,
      .padding = padding,
      .size = size,
      .error_cost = 0,
      .child_count = 0,
      .symbol = symbol,
      .parse_state = parse_state,
      .visible = metadata.visible,
      .named = metadata.named,
      .extra = extra,
      .has_external_tokens = has_external_tokens,
      .has_external_scanner_state_change = false,
      .is_missing = false,
      .is_keyword = is_keyword,
      .is_keyword_exact = is_keyword_exact,
      .trailing_extra = false,
      .first_child_has_completed_prefix = false,
    };
    return (Subtree) {.ptr = data};
  }
}

void ts_subtree_set_symbol(
  MutableSubtree *self,
  TSSymbol symbol,
  const TSLanguage *language
) {
  TSSymbolMetadata metadata = ts_language_symbol_metadata(language, symbol);
  if (self->data.is_inline) {
    ts_assert(symbol <= TS_MAX_INLINE_TREE_SYMBOL);
    self->data.symbol = (uint8_t)symbol;
    self->data.symbol_msb = (uint8_t)(symbol >> 8);
    self->data.named = metadata.named;
    self->data.visible = metadata.visible;
  } else {
    self->ptr->symbol = symbol;
    self->ptr->named = metadata.named;
    self->ptr->visible = metadata.visible;
  }
}

Subtree ts_subtree_new_error(
  SubtreePool *pool, int32_t lookahead_char, Length padding, Length size,
  TSStateId parse_state, const TSLanguage *language
) {
  Subtree result = ts_subtree_new_leaf(
    pool, ts_builtin_sym_error, padding, size,
    parse_state, false, false, false, language
  );
  SubtreeHeapData *data = (SubtreeHeapData *)result.ptr;
  data->lookahead_char = lookahead_char;
  return result;
}

// Clone a subtree.
MutableSubtree ts_subtree_clone(Subtree self) {
  size_t alloc_size = ts_subtree_alloc_size(self.ptr->child_count);
  Subtree *new_children = ts_malloc(alloc_size);
  Subtree *old_children = ts_subtree_children(self);
  memcpy(new_children, old_children, alloc_size);
  SubtreeHeapData *result = (SubtreeHeapData *)&new_children[self.ptr->child_count];
  if (self.ptr->child_count > 0) {
    for (uint32_t i = 0; i < self.ptr->child_count; i++) {
      ts_subtree_retain(new_children[i]);
    }
  } else if (self.ptr->has_external_tokens) {
    result->external_scanner_state = ts_external_scanner_state_copy(
      &self.ptr->external_scanner_state
    );
  }
  result->ref_count = 1;
  return (MutableSubtree) {.ptr = result};
}

// Get mutable version of a subtree.
//
// This takes ownership of the subtree. If the subtree has only one owner,
// this will directly convert it into a mutable version. Otherwise, it will
// perform a copy.
MutableSubtree ts_subtree_make_mut(SubtreePool *pool, Subtree self) {
  if (self.data.is_inline) return (MutableSubtree) {self.data};
  if (atomic_refcount_load_acquire(&self.ptr->ref_count) == 1) return ts_subtree_to_mut_unsafe(self);
  MutableSubtree result = ts_subtree_clone(self);
  ts_subtree_release(pool, self);
  return result;
}

void ts_subtree_compress(
  MutableSubtree self,
  unsigned count,
  const TSLanguage *language,
  MutableSubtreeArray *stack
) {
  unsigned initial_stack_size = stack->size;

  MutableSubtree tree = self;
  TSSymbol symbol = tree.ptr->symbol;
  for (unsigned i = 0; i < count; i++) {
    if (tree.ptr->ref_count > 1 || tree.ptr->child_count < 2) break;

    MutableSubtree child = ts_subtree_to_mut_unsafe(ts_subtree_children(tree)[0]);
    if (
      child.data.is_inline ||
      child.ptr->child_count < 2 ||
      child.ptr->ref_count > 1 ||
      child.ptr->symbol != symbol
    ) break;

    MutableSubtree grandchild = ts_subtree_to_mut_unsafe(ts_subtree_children(child)[0]);
    if (
      grandchild.data.is_inline ||
      grandchild.ptr->child_count < 2 ||
      grandchild.ptr->ref_count > 1 ||
      grandchild.ptr->symbol != symbol
    ) break;

    ts_subtree_children(tree)[0] = ts_subtree_from_mut(grandchild);
    ts_subtree_children(child)[0] = ts_subtree_children(grandchild)[grandchild.ptr->child_count - 1];
    ts_subtree_children(grandchild)[grandchild.ptr->child_count - 1] = ts_subtree_from_mut(child);
    array_push(stack, tree);
    tree = grandchild;
  }

  while (stack->size > initial_stack_size) {
    tree = array_pop(stack);
    MutableSubtree child = ts_subtree_to_mut_unsafe(ts_subtree_children(tree)[0]);
    MutableSubtree grandchild = ts_subtree_to_mut_unsafe(ts_subtree_children(child)[child.ptr->child_count - 1]);
    ts_subtree_summarize_children(grandchild, language);
    ts_subtree_summarize_children(child, language);
    ts_subtree_summarize_children(tree, language);
  }
}

// What `ts_subtree_summarize_children` accumulates while it visits a node's children in order.
typedef struct {
  const TSLanguage *language;
  const TSSymbol *alias_sequence;
  uint32_t structural_index;
  bool charges_error_extent;
} ChildSummary;

static inline void ts_subtree__summarize_extent(
  SubtreeHeapData *node,
  bool is_first,
  Length padding,
  Length size
) {
  if (is_first) {
    node->padding = padding;
    node->size = size;
  } else {
    node->size = length_add(node->size, length_add(padding, size));
  }
}

// Count the child when an alias or its own symbol makes it visible, and report whether it did.
static inline bool ts_subtree__summarize_visibility(
  SubtreeHeapData *node,
  const ChildSummary *summary,
  TSSymbol symbol,
  bool extra,
  bool visible,
  bool named
) {
  TSSymbol alias = !extra && symbol != 0 && summary->alias_sequence
    ? summary->alias_sequence[summary->structural_index]
    : 0;
  if (alias) {
    named = ts_language_symbol_metadata(summary->language, alias).named;
  } else if (!visible) {
    return false;
  }
  node->visible_descendant_count++;
  node->visible_child_count++;
  if (named) node->named_child_count++;
  return true;
}

static inline void ts_subtree__summarize_inline_child(
  SubtreeHeapData *node,
  const ChildSummary *summary,
  SubtreeInlineData child,
  bool is_first
) {
  Length padding = {child.padding_bytes, {child.padding_rows, child.padding_columns}};
  Length size = {child.size_bytes, {0, child.size_bytes}};
  ts_subtree__summarize_extent(node, is_first, padding, size);
  if (child.is_missing) node->error_cost += ERROR_COST_PER_MISSING_TREE + ERROR_COST_PER_RECOVERY;
  if (summary->charges_error_extent && !child.extra && child.visible) {
    node->error_cost += ERROR_COST_PER_SKIPPED_TREE;
  }
  TSSymbol symbol = (TSSymbol)child.symbol | ((TSSymbol)child.symbol_msb << 8);
  ts_subtree__summarize_visibility(node, summary, symbol, child.extra, child.visible, child.named);
  if (padding.bytes || size.bytes || child.extra) node->trailing_extra = child.extra;
}

static inline void ts_subtree__summarize_error_cost(
  SubtreeHeapData *node,
  const ChildSummary *summary,
  Subtree child
) {
  const SubtreeHeapData *data = child.ptr;
  uint32_t grandchild_count = data->child_count;
  if (data->symbol == ts_builtin_sym_error_repeat && !data->extra) {
    // Refund an `_ERROR` child's extent penalty, which this node re-charges
    // as part of its own extent below, so that the grouping is cost-neutral.
    uint32_t extent_cost = ts_subtree__error_extent_cost(data->size);
    ts_assert(ts_subtree_error_cost(child) >= extent_cost);
    node->error_cost += ts_subtree_error_cost(child) - extent_cost;
  } else {
    node->error_cost += ts_subtree_error_cost(child);
    if (
      summary->charges_error_extent && !data->extra &&
      !(data->symbol == ts_builtin_sym_error && grandchild_count == 0)
    ) {
      if (data->visible) {
        node->error_cost += ERROR_COST_PER_SKIPPED_TREE;
      } else if (grandchild_count > 0) {
        node->error_cost += ERROR_COST_PER_SKIPPED_TREE * data->visible_child_count;
      }
    }
  }
}

static void ts_subtree__summarize_heap_child(
  SubtreeHeapData *node,
  const ChildSummary *summary,
  Subtree child,
  bool is_first
) {
  const SubtreeHeapData *data = child.ptr;
  ts_subtree__summarize_extent(node, is_first, data->padding, data->size);

  ts_subtree__summarize_error_cost(node, summary, child);

  uint32_t grandchild_count = data->child_count;
  node->dynamic_precedence += ts_subtree_dynamic_precedence(child);
  node->visible_descendant_count += ts_subtree_visible_descendant_count(child);
  if (
    !ts_subtree__summarize_visibility(node, summary, data->symbol, data->extra, data->visible, data->named) &&
    grandchild_count > 0
  ) {
    node->visible_child_count += data->visible_child_count;
    node->named_child_count += data->named_child_count;
  }

  if (data->has_external_tokens) node->has_external_tokens = true;
  if (ts_subtree_total_bytes(child) || data->has_external_tokens || data->extra) {
    node->trailing_extra = ts_subtree_trailing_extra(child);
  }

  if (data->symbol == ts_builtin_sym_error) {
    node->parse_state = TS_TREE_STATE_NONE;
  }
}

static inline void ts_subtree__summarize_ends(
  SubtreeHeapData *node,
  Subtree first_child,
  Subtree last_child
) {
  node->first_child_has_completed_prefix = ts_subtree_has_completed_prefix(first_child);

  if (
    node->child_count >= 2 &&
    !node->is_extra_group &&
    !node->visible &&
    !node->named &&
    ts_subtree_symbol(first_child) == node->symbol
  ) {
    uint16_t first_depth = ts_subtree_repeat_depth(first_child);
    uint16_t last_depth = ts_subtree_repeat_depth(last_child);
    node->repeat_depth = (first_depth > last_depth ? first_depth : last_depth) + 1;
  }
}

static void ts_subtree__reset_child_summary_fields(SubtreeHeapData *node) {
  node->named_child_count = 0;
  node->visible_child_count = 0;
  node->error_cost = 0;
  if (!node->is_extra_group) node->repeat_depth = 0;
  node->visible_descendant_count = 0;
  node->has_external_tokens = false;
  node->trailing_extra = false;
  node->dynamic_precedence = 0;
}

// Assign all of the node's properties that depend on its children.
void ts_subtree_summarize_children(
  MutableSubtree self,
  const TSLanguage *language
) {
  ts_assert(!self.data.is_inline);
  ts_subtree__reset_child_summary_fields(self.ptr);

  bool charges_error_extent = self.ptr->symbol == ts_builtin_sym_error ||
    (self.ptr->symbol == ts_builtin_sym_error_repeat && !self.ptr->extra);
  ChildSummary summary = {
    .language = language,
    .alias_sequence = ts_language_alias_sequence(language, ts_subtree_production_id(ts_subtree_from_mut(self))),
    .structural_index = 0,
    .charges_error_extent = charges_error_extent,
  };

  const Subtree *children = ts_subtree_children(self);
  for (uint32_t i = 0; i < self.ptr->child_count; i++) {
    Subtree child = children[i];
    if (child.data.is_inline) {
      ts_subtree__summarize_inline_child(self.ptr, &summary, child.data, i == 0);
    } else {
      ts_subtree__summarize_heap_child(self.ptr, &summary, child, i == 0);
    }
    if (!ts_subtree_extra(child)) summary.structural_index++;
  }

  if (charges_error_extent) {
    self.ptr->error_cost += ts_subtree__error_extent_cost(self.ptr->size);
  }

  if (self.ptr->child_count > 0) {
    ts_subtree__summarize_ends(self.ptr, children[0], children[self.ptr->child_count - 1]);
  }
}

// Takes ownership of both subtrees.
Subtree ts_subtree_new_extra_group(Subtree left, Subtree right, const TSLanguage *language) {
  ts_assert(ts_subtree_extra(left) && ts_subtree_extra(right));
  SubtreeArray children = array_new();
  array_push(&children, left);
  array_push(&children, right);
  MutableSubtree result = ts_subtree_new_node(ts_builtin_sym_error_repeat, &children, 0, language);
  result.ptr->extra = true;
  result.ptr->is_extra_group = true;
  result.ptr->stack_link_count = ts_subtree_stack_link_count(left) + ts_subtree_stack_link_count(right);
  ts_subtree_summarize_children(result, language);
  return ts_subtree_from_mut(result);
}

// Create a new parent node with the given children.
//
// This takes ownership of the children array.
MutableSubtree ts_subtree_new_node(
  TSSymbol symbol,
  SubtreeArray *children,
  unsigned production_id,
  const TSLanguage *language
) {
  TSSymbolMetadata metadata = ts_language_symbol_metadata(language, symbol);

  // Allocate the node's data at the end of the array of children.
  size_t new_byte_size = ts_subtree_alloc_size(children->size);
  if (children->capacity * sizeof(Subtree) < new_byte_size) {
    children->contents = ts_realloc(children->contents, new_byte_size);
    children->capacity = (uint32_t)(new_byte_size / sizeof(Subtree));
  }
  SubtreeHeapData *data = (SubtreeHeapData *)&children->contents[children->size];

  *data = (SubtreeHeapData) {
    .ref_count = 1,
    .symbol = symbol,
    .child_count = children->size,
    .visible = metadata.visible,
    .named = metadata.named,
    .has_external_scanner_state_change = false,
    .is_keyword = false,
    .is_keyword_exact = false,
    .trailing_extra = false,
    .first_child_has_completed_prefix = false,
    .is_extra_group = false,
    .visible_descendant_count = 0,
    .production_id = production_id,
  };
  MutableSubtree result = {.ptr = data};
  ts_subtree_summarize_children(result, language);
  return result;
}

// Create a new error node containing the given children.
//
// This node is treated as 'extra'. Its children are prevented from having
// any effect on the parse state.
Subtree ts_subtree_new_error_node(
  SubtreeArray *children,
  bool extra,
  const TSLanguage *language
) {
  MutableSubtree result = ts_subtree_new_node(
    ts_builtin_sym_error, children, 0, language
  );
  result.ptr->extra = extra;
  return ts_subtree_from_mut(result);
}

// Create a new 'missing leaf' node.
//
// This node is treated as 'extra'. Its children are prevented from having
// any effect on the parse state.
Subtree ts_subtree_new_missing_leaf(
  SubtreePool *pool,
  TSSymbol symbol,
  TSStateId state,
  Length padding,
  const TSLanguage *language
) {
  Subtree result = ts_subtree_new_leaf(
    pool, symbol, padding, length_zero(),
    state, false, false, false, language
  );
  if (result.data.is_inline) {
    result.data.is_missing = true;
  } else {
    ((SubtreeHeapData *)result.ptr)->is_missing = true;
  }
  return result;
}

void ts_subtree_retain(Subtree self) {
  if (self.data.is_inline) return;
  ts_assert(self.ptr->ref_count > 0);
  uint32_t count = atomic_refcount_inc((volatile uint32_t *)&self.ptr->ref_count);
  ts_assert(count != 0);
  (void)count;
}

void ts_subtree_release(SubtreePool *pool, Subtree self) {
  if (self.data.is_inline) return;
  ts_assert(self.ptr->ref_count > 0);
  if (!atomic_refcount_dec_is_last((volatile uint32_t *)&self.ptr->ref_count)) return;

  array_clear(&pool->tree_stack);
  array_push(&pool->tree_stack, ts_subtree_to_mut_unsafe(self));
  while (pool->tree_stack.size > 0) {
    MutableSubtree tree = array_pop(&pool->tree_stack);
    if (tree.ptr->child_count == 0) {
      if (tree.ptr->has_external_tokens) ts_external_scanner_state_delete(&tree.ptr->external_scanner_state);
      ts_subtree_pool_free(pool, tree.ptr);
      continue;
    }
    Subtree *children = ts_subtree_children(tree);
    for (uint32_t i = 0; i < tree.ptr->child_count; i++) {
      Subtree child = children[i];
      if (!child.data.is_inline && atomic_refcount_dec_is_last((volatile uint32_t *)&child.ptr->ref_count)) {
        array_push(&pool->tree_stack, ts_subtree_to_mut_unsafe(child));
      }
    }
    ts_free(children);
  }
}

int ts_subtree_compare(Subtree left, Subtree right, SubtreePool *pool) {
  array_push(&pool->tree_stack, ts_subtree_to_mut_unsafe(left));
  array_push(&pool->tree_stack, ts_subtree_to_mut_unsafe(right));

  while (pool->tree_stack.size > 0) {
    right = ts_subtree_from_mut(array_pop(&pool->tree_stack));
    left = ts_subtree_from_mut(array_pop(&pool->tree_stack));

    int result = 0;
    if (ts_subtree_symbol(left) < ts_subtree_symbol(right)) result = -1;
    else if (ts_subtree_symbol(right) < ts_subtree_symbol(left)) result = 1;
    else if (ts_subtree_child_count(left) < ts_subtree_child_count(right)) result = -1;
    else if (ts_subtree_child_count(right) < ts_subtree_child_count(left)) result = 1;
    if (result != 0) {
      array_clear(&pool->tree_stack);
      return result;
    }

    for (uint32_t i = ts_subtree_child_count(left); i > 0; i--) {
      Subtree left_child = ts_subtree_children(left)[i - 1];
      Subtree right_child = ts_subtree_children(right)[i - 1];
      array_push(&pool->tree_stack, ts_subtree_to_mut_unsafe(left_child));
      array_push(&pool->tree_stack, ts_subtree_to_mut_unsafe(right_child));
    }
  }

  return 0;
}

Subtree ts_subtree_last_external_token(Subtree tree) {
  if (!ts_subtree_has_external_tokens(tree)) return NULL_SUBTREE;
  while (tree.ptr->child_count > 0) {
    for (uint32_t i = tree.ptr->child_count - 1; i + 1 > 0; i--) {
      Subtree child = ts_subtree_children(tree)[i];
      if (ts_subtree_has_external_tokens(child)) {
        tree = child;
        break;
      }
    }
  }
  return tree;
}

static size_t ts_subtree__write_char_to_string(char *str, size_t n, int32_t chr) {
  if (chr == -1)
    return snprintf(str, n, "INVALID");
  else if (chr == '\0')
    return snprintf(str, n, "'\\0'");
  else if (chr == '\n')
    return snprintf(str, n, "'\\n'");
  else if (chr == '\t')
    return snprintf(str, n, "'\\t'");
  else if (chr == '\r')
    return snprintf(str, n, "'\\r'");
  else if (0 < chr && chr < 128 && isprint(chr))
    return snprintf(str, n, "'%c'", chr);
  else
    return snprintf(str, n, "%d", chr);
}

static const char *const ROOT_FIELD = "__ROOT__";

typedef struct {
  Subtree subtree;
  TSSymbol alias_symbol;
  bool alias_is_named;
  const char *field_name;
  bool is_root;

  bool pre_written;
  bool is_visible;
  uint32_t child_index;
  uint32_t structural_child_index;
  const TSSymbol *alias_sequence;
  const TSFieldMapEntry *field_map;
  const TSFieldMapEntry *field_map_end;
} WriteToStringFrame;

static size_t ts_subtree__write_indent(char *string, size_t limit, size_t indent_level);

// code-gauge-allow functionParameterCount 8 -- iterative S-expression formatter threads output buffer and root metadata
static size_t ts_subtree__write_to_string(
  Subtree self, char *string, size_t limit,
  const TSLanguage *language, SubtreeStringOptions options,
  TSSymbol root_alias_symbol, bool root_alias_is_named, const char *root_field_name
) {
  char *cursor = string;
  char **writer = (limit > 1) ? &cursor : &string;
  size_t indent_level = options.initial_indent_level;
  if (options.pretty && indent_level > 0) {
    cursor += ts_subtree__write_indent(*writer, limit, indent_level);
  }

  Array(WriteToStringFrame) stack = array_new();
  array_push(&stack, ((WriteToStringFrame) {
    .subtree = self,
    .alias_symbol = root_alias_symbol,
    .alias_is_named = root_alias_is_named,
    .field_name = root_field_name,
    .is_root = root_field_name == ROOT_FIELD,
  }));

  while (stack.size) {
    WriteToStringFrame *frame = array_back(&stack);
    Subtree node = frame->subtree;

    if (!node.ptr) {
      if (!frame->is_root) {
        cursor += options.pretty
          ? ts_subtree__write_indent(*writer, limit, indent_level)
          : snprintf(*writer, limit, " ");
        if (frame->field_name) {
          cursor += snprintf(*writer, limit, "%s: ", frame->field_name);
        }
      }
      cursor += snprintf(*writer, limit, "(NULL)");
      (void)array_pop(&stack);
      continue;
    }

    if (!frame->pre_written) {
      bool is_visible =
        options.include_all ||
        ts_subtree_missing(node) ||
        (
          frame->alias_symbol
            ? frame->alias_is_named
            : ts_subtree_visible(node) && ts_subtree_named(node)
        );

      if (is_visible) {
        if (!frame->is_root) {
          cursor += options.pretty
            ? ts_subtree__write_indent(*writer, limit, indent_level)
            : snprintf(*writer, limit, " ");
          if (frame->field_name) {
            cursor += snprintf(*writer, limit, "%s: ", frame->field_name);
          }
        }

        if (ts_subtree_is_error(node) && ts_subtree_child_count(node) == 0 && node.ptr->size.bytes > 0) {
          cursor += snprintf(*writer, limit, "(UNEXPECTED ");
          cursor += ts_subtree__write_char_to_string(*writer, limit, node.ptr->lookahead_char);
        } else {
          TSSymbol symbol = frame->alias_symbol ? frame->alias_symbol : ts_subtree_symbol(node);
          const char *symbol_name = ts_language_symbol_name(language, symbol);
          if (ts_subtree_missing(node)) {
            cursor += snprintf(*writer, limit, "(MISSING ");
            if (frame->alias_is_named || ts_subtree_named(node)) {
              cursor += snprintf(*writer, limit, "%s", symbol_name);
            } else {
              cursor += snprintf(*writer, limit, "\"%s\"", symbol_name);
            }
          } else {
            cursor += snprintf(*writer, limit, "(%s", symbol_name);
          }
        }
      } else if (frame->is_root) {
        TSSymbol symbol = frame->alias_symbol ? frame->alias_symbol : ts_subtree_symbol(node);
        const char *symbol_name = ts_language_symbol_name(language, symbol);
        if (ts_subtree_child_count(node) > 0) {
          cursor += snprintf(*writer, limit, "(%s", symbol_name);
        } else if (ts_subtree_named(node)) {
          cursor += snprintf(*writer, limit, "(%s)", symbol_name);
        } else {
          cursor += snprintf(*writer, limit, "(\"%s\")", symbol_name);
        }
      }

      if (ts_subtree_child_count(node)) {
        uint16_t production_id = ts_subtree_production_id(node);
        frame->alias_sequence = ts_language_alias_sequence(language, production_id);
        ts_language_field_map(
          language,
          production_id,
          &frame->field_map,
          &frame->field_map_end
        );
      }

      frame->is_visible = is_visible;
      if (options.pretty && is_visible) indent_level++;
      frame->pre_written = true;
    }

    if (frame->child_index < ts_subtree_child_count(node)) {
      Subtree child = ts_subtree_children(node)[frame->child_index];
      WriteToStringFrame child_frame = {
        .subtree = child,
        .is_root = false,
      };

      if (ts_subtree_extra(child)) {
        // Extra children carry no alias/field info.
      } else {
        TSSymbol subtree_alias_symbol = frame->alias_sequence
          ? frame->alias_sequence[frame->structural_child_index]
          : 0;
        bool subtree_alias_is_named = subtree_alias_symbol
          ? ts_language_symbol_metadata(language, subtree_alias_symbol).named
          : false;

        const char *child_field_name = frame->is_visible ? NULL : frame->field_name;
        for (const TSFieldMapEntry *map = frame->field_map; map < frame->field_map_end; map++) {
          if (!map->inherited && map->child_index == frame->structural_child_index) {
            child_field_name = language->field_names[map->field_id];
            break;
          }
        }

        child_frame.alias_symbol = subtree_alias_symbol;
        child_frame.alias_is_named = subtree_alias_is_named;
        child_frame.field_name = child_field_name;
        frame->structural_child_index++;
      }

      frame->child_index++;
      // After this push, `frame` may be invalidated by a realloc.
      array_push(&stack, child_frame);
      continue;
    }

    if (frame->is_visible) {
      cursor += snprintf(*writer, limit, ")");
      if (options.pretty) indent_level--;
    }
    (void)array_pop(&stack);
  }

  array_delete(&stack);
  return cursor - string;
}

static size_t ts_subtree__write_indent(char *string, size_t limit, size_t indent_level) {
  size_t size = snprintf(string, limit, "\n");
  for (size_t i = 0; i < indent_level; i++) {
    char *position = limit > 1 ? string + size : string;
    size += snprintf(position, limit, "  ");
  }
  return size;
}

char *ts_subtree_string(
  Subtree self,
  TSSymbol alias_symbol,
  bool alias_is_named,
  const TSLanguage *language,
  SubtreeStringOptions options
) {
  char scratch_string[1];
  size_t size = ts_subtree__write_to_string(
    self, scratch_string, 1,
    language, options,
    alias_symbol, alias_is_named, ROOT_FIELD
  ) + 1;
  char *result = ts_malloc(size * sizeof(char));
  ts_subtree__write_to_string(
    self, result, size,
    language, options,
    alias_symbol, alias_is_named, ROOT_FIELD
  );
  return result;
}

typedef struct {
  const Subtree *subtree;
  uint32_t start_offset;
  uint32_t child_start_offset;
  uint32_t child_info_offset;
  uint32_t child_index;
  TSSymbol alias_symbol;
} PrintDotGraphFrame;

static void ts_subtree__write_dot_node(
  const Subtree *self,
  uint32_t start_offset,
  const TSLanguage *language,
  TSSymbol alias_symbol,
  FILE *f
);

void ts_subtree__print_dot_graph(const Subtree *self, uint32_t start_offset,
                                 const TSLanguage *language, TSSymbol alias_symbol,
                                 FILE *f) {
  Array(PrintDotGraphFrame) stack = array_new();
  array_push(&stack, ((PrintDotGraphFrame) {
    .subtree = self,
    .start_offset = start_offset,
    .child_start_offset = start_offset,
    .child_info_offset = ts_language_alias_sequence_offset(language, ts_subtree_production_id(*self)),
    .alias_symbol = alias_symbol,
  }));

  while (stack.size) {
    PrintDotGraphFrame *frame = array_back(&stack);
    self = frame->subtree;
    if (frame->child_index == 0) {
      ts_subtree__write_dot_node(self, frame->start_offset, language, frame->alias_symbol, f);
    }

    if (frame->child_index < ts_subtree_child_count(*self)) {
      const Subtree *child = &ts_subtree_children(*self)[frame->child_index];
      TSSymbol child_alias_symbol = 0;
      if (!ts_subtree_extra(*child) && frame->child_info_offset) {
        child_alias_symbol = language->alias_sequences[frame->child_info_offset++];
      }
      uint32_t child_start_offset = frame->child_start_offset;
      frame->child_start_offset += ts_subtree_total_bytes(*child);
      frame->child_index++;
      array_push(&stack, ((PrintDotGraphFrame) {
        .subtree = child,
        .start_offset = child_start_offset,
        .child_start_offset = child_start_offset,
        .child_info_offset = ts_language_alias_sequence_offset(language, ts_subtree_production_id(*child)),
        .alias_symbol = child_alias_symbol,
      }));
    } else {
      (void)array_pop(&stack);
      if (stack.size) {
        frame = array_back(&stack);
        fprintf(f, "tree_%p -> tree_%p [tooltip=%u]\n",
          (void *)frame->subtree, (void *)self, frame->child_index - 1);
      }
    }
  }

  array_delete(&stack);
}

static void ts_subtree__write_dot_node(
  const Subtree *self,
  uint32_t start_offset,
  const TSLanguage *language,
  TSSymbol alias_symbol,
  FILE *f
) {
  TSSymbol subtree_symbol = ts_subtree_symbol(*self);
  TSSymbol symbol = alias_symbol ? alias_symbol : subtree_symbol;
  uint32_t end_offset = start_offset + ts_subtree_total_bytes(*self);
  fprintf(f, "tree_%p [label=\"", (void *)self);
  ts_language_write_symbol_as_dot_string(language, f, symbol);
  fprintf(f, "\"");

  if (ts_subtree_child_count(*self) == 0) fprintf(f, ", shape=plaintext");
  if (ts_subtree_extra(*self)) fprintf(f, ", fontcolor=gray");

  fprintf(f, ", tooltip=\""
    "range: %u - %u\n"
    "state: %d\n"
    "error-cost: %u\n"
    "descendant-count: %u\n"
    "repeat-depth: %u",
    start_offset, end_offset,
    ts_subtree_parse_state(*self),
    ts_subtree_error_cost(*self),
    ts_subtree_visible_descendant_count(*self),
    ts_subtree_repeat_depth(*self)
  );

  if (ts_subtree_is_error(*self) && ts_subtree_child_count(*self) == 0 && self->ptr->lookahead_char != 0) {
    fprintf(f, "\ncharacter: '%c'", self->ptr->lookahead_char);
  }

  fprintf(f, "\"]\n");
}

void ts_subtree_print_dot_graph(Subtree self, const TSLanguage *language, FILE *f) {
  fprintf(f, "digraph tree {\n");
  fprintf(f, "edge [arrowhead=none]\n");
  ts_subtree__print_dot_graph(&self, 0, language, 0, f);
  fprintf(f, "}\n");
}

const ExternalScannerState *ts_subtree_external_scanner_state(Subtree self) {
  static const ExternalScannerState empty_state = {.data = {0}, .length = 0};
  if (
    self.ptr &&
    !self.data.is_inline &&
    self.ptr->has_external_tokens &&
    self.ptr->child_count == 0
  ) {
    return &self.ptr->external_scanner_state;
  } else {
    return &empty_state;
  }
}

bool ts_subtree_external_scanner_state_eq(Subtree self, Subtree other) {
  const ExternalScannerState *state_self = ts_subtree_external_scanner_state(self);
  const ExternalScannerState *state_other = ts_subtree_external_scanner_state(other);
  return ts_external_scanner_state_eq(
    state_self,
    ts_external_scanner_state_data(state_other),
    state_other->length
  );
}
