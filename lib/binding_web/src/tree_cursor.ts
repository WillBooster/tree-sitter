import { INTERNAL, Internal, assertInternal, Point, C } from './constants';
import { marshalNode, unmarshalNode, unmarshalPoint } from './marshal';
import { Node } from './node';
import { TRANSFER_BUFFER } from './parser';
import { getText, Tree } from './tree';
import { newFinalizer } from './finalization_registry';

const finalizer = newFinalizer((address: number) => {
  C._ts_tree_cursor_delete_wasm(address);
});

/** A stateful object for walking a syntax {@link Tree} efficiently. */
export class TreeCursor {
  /** @internal */
  private [0] = 0; // Internal handle for Wasm

  /** @internal */
  private tree: Tree;

  /** @internal */
  constructor(internal: Internal, tree: Tree, address: number) {
    assertInternal(internal);
    this[0] = address;
    this.tree = tree;
    finalizer?.register(this, address, this);
  }

  /** Creates a deep copy of the tree cursor. This allocates new memory. */
  copy(): TreeCursor {
    const address = C._ts_tree_cursor_copy_wasm(this[0]);
    return new TreeCursor(INTERNAL, this.tree, address);
  }

  /** Delete the tree cursor, freeing its resources. */
  delete(): void {
    finalizer?.unregister(this);
    C._ts_tree_cursor_delete_wasm(this[0]);
    this[0] = 0;
  }

  /** Get the tree cursor's current {@link Node}. */
  get currentNode(): Node {
    C._ts_tree_cursor_current_node_wasm(this[0]);
    return unmarshalNode(this.tree)!;
  }

  /**
   * Get the numerical field id of this tree cursor's current node.
   *
   * See also {@link TreeCursor#currentFieldName}.
   */
  get currentFieldId(): number {
    return C._ts_tree_cursor_current_field_id(this[0]);
  }

  /** Get the field name of this tree cursor's current node. */
  get currentFieldName(): string | null {
    return this.tree.language.fields[this.currentFieldId];
  }

  /**
   * Get the depth of the cursor's current node relative to the original
   * node that the cursor was constructed with.
   */
  get currentDepth(): number {
    return C._ts_tree_cursor_current_depth(this[0]);
  }

  /**
   * Get the index of the cursor's current node out of all of the
   * descendants of the original node that the cursor was constructed with.
   */
  get currentDescendantIndex(): number {
    return C._ts_tree_cursor_current_descendant_index(this[0]);
  }

  /** Get the type of the cursor's current node. */
  get nodeType(): string {
    return this.tree.language.types[this.nodeTypeId] || 'ERROR';
  }

  /** Get the type id of the cursor's current node. */
  get nodeTypeId(): number {
    return C._ts_tree_cursor_current_node_type_id_wasm(this[0]);
  }

  /** Get the state id of the cursor's current node. */
  get nodeStateId(): number {
    return C._ts_tree_cursor_current_node_state_id_wasm(this[0]);
  }

  /** Get the id of the cursor's current node. */
  get nodeId(): number {
    return C._ts_tree_cursor_current_node_id_wasm(this[0]);
  }

  /**
   * Check if the cursor's current node is *named*.
   *
   * Named nodes correspond to named rules in the grammar, whereas
   * *anonymous* nodes correspond to string literals in the grammar.
   */
  get nodeIsNamed(): boolean {
    return C._ts_tree_cursor_current_node_is_named_wasm(this[0]) === 1;
  }

  /**
   * Check if the cursor's current node is *missing*.
   *
   * Missing nodes are inserted by the parser in order to recover from
   * certain kinds of syntax errors.
   */
  get nodeIsMissing(): boolean {
    return C._ts_tree_cursor_current_node_is_missing_wasm(this[0]) === 1;
  }

  /** Get the string content of the cursor's current node. */
  get nodeText(): string {
    const { textSource } = this.tree;
    if (typeof textSource === 'string') {
      C._ts_tree_cursor_text_range_wasm(this[0], 0);
      const idx = TRANSFER_BUFFER >> 2;
      const heap = C.HEAPU32;
      return textSource.slice(heap[idx], heap[idx + 1]);
    }
    C._ts_tree_cursor_text_range_wasm(this[0], 1);
    const idx = TRANSFER_BUFFER >> 2;
    const heap = C.HEAPU32;
    return getText(this.tree, heap[idx], heap[idx + 1], { row: heap[idx + 2], column: heap[idx + 3] });
  }

  /** Get the start position of the cursor's current node. */
  get startPosition(): Point {
    C._ts_tree_cursor_start_position_wasm(this[0]);
    return unmarshalPoint(TRANSFER_BUFFER);
  }

  /** Get the end position of the cursor's current node. */
  get endPosition(): Point {
    C._ts_tree_cursor_end_position_wasm(this[0]);
    return unmarshalPoint(TRANSFER_BUFFER);
  }

  /** Get the start index of the cursor's current node. */
  get startIndex(): number {
    return C._ts_tree_cursor_start_index_wasm(this[0]);
  }

  /** Get the end index of the cursor's current node. */
  get endIndex(): number {
    return C._ts_tree_cursor_end_index_wasm(this[0]);
  }

  /**
   * Move this cursor to the first child of its current node.
   *
   * This returns `true` if the cursor successfully moved, and returns
   * `false` if there were no children.
   */
  gotoFirstChild(): boolean {
    return C._ts_tree_cursor_goto_first_child(this[0]) === 1;
  }

  /**
   * Move this cursor to the last child of its current node.
   *
   * This returns `true` if the cursor successfully moved, and returns
   * `false` if there were no children.
   *
   * Note that this function may be slower than
   * {@link TreeCursor#gotoFirstChild} because it needs to
   * iterate through all the children to compute the child's position.
   */
  gotoLastChild(): boolean {
    return C._ts_tree_cursor_goto_last_child(this[0]) === 1;
  }

  /**
   * Move this cursor to the parent of its current node.
   *
   * This returns `true` if the cursor successfully moved, and returns
   * `false` if there was no parent node (the cursor was already on the
   * root node).
   *
   * Note that the node the cursor was constructed with is considered the root
   * of the cursor, and the cursor cannot walk outside this node.
   */
  gotoParent(): boolean {
    return C._ts_tree_cursor_goto_parent(this[0]) === 1;
  }

  /**
   * Move this cursor to the next sibling of its current node.
   *
   * This returns `true` if the cursor successfully moved, and returns
   * `false` if there was no next sibling node.
   *
   * Note that the node the cursor was constructed with is considered the root
   * of the cursor, and the cursor cannot walk outside this node.
   */
  gotoNextSibling(): boolean {
    return C._ts_tree_cursor_goto_next_sibling(this[0]) === 1;
  }

  /**
   * Move this cursor to the previous sibling of its current node.
   *
   * This returns `true` if the cursor successfully moved, and returns
   * `false` if there was no previous sibling node.
   *
   * Note that this function may be slower than
   * {@link TreeCursor#gotoNextSibling} due to how node
   * positions are stored. In the worst case, this will need to iterate
   * through all the children up to the previous sibling node to recalculate
   * its position. Also note that the node the cursor was constructed with is
   * considered the root of the cursor, and the cursor cannot walk outside this node.
   */
  gotoPreviousSibling(): boolean {
    return C._ts_tree_cursor_goto_previous_sibling(this[0]) === 1;
  }

  /**
   * Move the cursor to the node that is the nth descendant of
   * the original node that the cursor was constructed with, where
   * zero represents the original node itself.
   */
  gotoDescendant(goalDescendantIndex: number): void {
    C._ts_tree_cursor_goto_descendant(this[0], goalDescendantIndex);
  }

  /**
   * Move this cursor to the first child of its current node that contains or
   * starts after the given index.
   *
   * This returns `true` if the cursor successfully moved to a child node, and returns
   * `false` if no such child was found.
   */
  gotoFirstChildForIndex(goalIndex: number): boolean {
    return C._ts_tree_cursor_goto_first_child_for_index_wasm(this[0], goalIndex) === 1;
  }

  /**
   * Move this cursor to the first child of its current node that contains or
   * starts after the given position.
   *
   * This returns `true` if the cursor successfully moved to a child node, and returns
   * `false` if no such child was found.
   */
  gotoFirstChildForPosition(goalPosition: Point): boolean {
    return C._ts_tree_cursor_goto_first_child_for_position_wasm(
      this[0],
      goalPosition.row,
      goalPosition.column,
    ) === 1;
  }

  /**
   * Re-initialize this tree cursor to start at the original node that the
   * cursor was constructed with.
   */
  reset(node: Node): void {
    marshalNode(node);
    C._ts_tree_cursor_reset_wasm(this[0], node.tree[0]);
    this.tree = node.tree;
  }

  /**
   * Re-initialize a tree cursor to the same position as another cursor.
   *
   * Unlike {@link TreeCursor#reset}, this will not lose parent
   * information and allows reusing already created cursors.
   */
  resetTo(cursor: TreeCursor): void {
    C._ts_tree_cursor_reset_to(this[0], cursor[0]);
    this.tree = cursor.tree;
  }
}
