import { INTERNAL, Internal, assertInternal, Point, C } from './constants';
import { marshalNode, marshalPoint, unmarshalNode, unmarshalPoint } from './marshal';
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
  private editVersion = -1;

  /** @internal */
  private _nodeTypeId = 0;

  /** @internal */
  private _fieldId = 0;

  /** @internal */
  private _startIndex = 0;

  /** @internal */
  private _endIndex = 0;

  /** @internal */
  private _flags = 0;

  /** @internal */
  constructor(internal: Internal, tree: Tree, address: number) {
    assertInternal(internal);
    this[0] = address;
    this.tree = tree;
    this.readNodeCache();
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
    return this._fieldId;
  }

  /** Get the field name of this tree cursor's current node. */
  get currentFieldName(): string | null {
    return this.tree.language.fields[this._fieldId];
  }

  /**
   * Get the depth of the cursor's current node relative to the original
   * node that the cursor was constructed with.
   */
  get currentDepth(): number {
    return C._ts_tree_cursor_current_depth_wasm(this[0]);
  }

  /**
   * Get the index of the cursor's current node out of all of the
   * descendants of the original node that the cursor was constructed with.
   */
  get currentDescendantIndex(): number {
    return C._ts_tree_cursor_current_descendant_index_wasm(this[0]);
  }

  /** Get the type of the cursor's current node. */
  get nodeType(): string {
    return this.tree.language.types[this._nodeTypeId] || 'ERROR';
  }

  /** Get the type id of the cursor's current node. */
  get nodeTypeId(): number {
    return this._nodeTypeId;
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
    return (this._flags & 1) !== 0;
  }

  /**
   * Check if the cursor's current node is *missing*.
   *
   * Missing nodes are inserted by the parser in order to recover from
   * certain kinds of syntax errors.
   */
  get nodeIsMissing(): boolean {
    return (this._flags & 2) !== 0;
  }

  /** Get the string content of the cursor's current node. */
  get nodeText(): string {
    this.syncIndices();
    return getText(this.tree, this._startIndex, this._endIndex, () => this.startPosition);
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
    this.syncIndices();
    return this._startIndex;
  }

  /** Get the end index of the cursor's current node. */
  get endIndex(): number {
    this.syncIndices();
    return this._endIndex;
  }

  /**
   * Move this cursor to the first child of its current node.
   *
   * This returns `true` if the cursor successfully moved, and returns
   * `false` if there were no children.
   */
  gotoFirstChild(): boolean {
    if (!C._ts_tree_cursor_goto_first_child_wasm(this[0])) return false;
    this.readNodeCache();
    return true;
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
    if (!C._ts_tree_cursor_goto_last_child_wasm(this[0])) return false;
    this.readNodeCache();
    return true;
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
    if (!C._ts_tree_cursor_goto_parent_wasm(this[0])) return false;
    this.readNodeCache();
    return true;
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
    if (!C._ts_tree_cursor_goto_next_sibling_wasm(this[0])) return false;
    this.readNodeCache();
    return true;
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
    if (!C._ts_tree_cursor_goto_previous_sibling_wasm(this[0])) return false;
    this.readNodeCache();
    return true;
  }

  /**
   * Move the cursor to the node that is the nth descendant of
   * the original node that the cursor was constructed with, where
   * zero represents the original node itself.
   */
  gotoDescendant(goalDescendantIndex: number): void {
    C._ts_tree_cursor_goto_descendant_wasm(this[0], goalDescendantIndex);
    this.readNodeCache();
  }

  /**
   * Move this cursor to the first child of its current node that contains or
   * starts after the given index.
   *
   * This returns `true` if the cursor successfully moved to a child node, and returns
   * `false` if no such child was found.
   */
  gotoFirstChildForIndex(goalIndex: number): boolean {
    if (!C._ts_tree_cursor_goto_first_child_for_index_wasm(this[0], goalIndex)) return false;
    this.readNodeCache();
    return true;
  }

  /**
   * Move this cursor to the first child of its current node that contains or
   * starts after the given position.
   *
   * This returns `true` if the cursor successfully moved to a child node, and returns
   * `false` if no such child was found.
   */
  gotoFirstChildForPosition(goalPosition: Point): boolean {
    marshalPoint(TRANSFER_BUFFER, goalPosition);
    if (!C._ts_tree_cursor_goto_first_child_for_position_wasm(this[0])) return false;
    this.readNodeCache();
    return true;
  }

  /**
   * Re-initialize this tree cursor to start at the original node that the
   * cursor was constructed with.
   */
  reset(node: Node): void {
    marshalNode(node);
    this.tree = node.tree;
    C._ts_tree_cursor_reset_wasm(this[0], node.tree[0]);
    this.readNodeCache();
  }

  /**
   * Re-initialize a tree cursor to the same position as another cursor.
   *
   * Unlike {@link TreeCursor#reset}, this will not lose parent
   * information and allows reusing already created cursors.
   */
  resetTo(cursor: TreeCursor): void {
    if (cursor === this) return;
    this.tree = cursor.tree;
    C._ts_tree_cursor_reset_to_wasm(this[0], cursor[0]);
    this.readNodeCache();
  }

  /** @internal */
  private readNodeCache(): void {
    const base = TRANSFER_BUFFER >>> 2;
    const heap = C.HEAPU32;
    this._nodeTypeId = heap[base];
    this._fieldId = heap[base + 1];
    this._startIndex = heap[base + 2];
    this._endIndex = heap[base + 3];
    this._flags = heap[base + 4];
    this.editVersion = this.tree.editVersion;
  }

  /** @internal */
  private syncIndices(): void {
    if (this.editVersion !== this.tree.editVersion) {
      this._startIndex = C._ts_tree_cursor_start_index_wasm(this[0]) >>> 0;
      this._endIndex = C._ts_tree_cursor_end_index_wasm(this[0]) >>> 0;
      this.editVersion = this.tree.editVersion;
    }
  }
}
