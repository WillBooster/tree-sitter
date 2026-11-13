import { INTERNAL, Internal, assertInternal, SIZE_OF_INT, SIZE_OF_NODE, SIZE_OF_POINT, ZERO_POINT, isPoint, C, Point } from './constants';
import { getCallbackText, Tree } from './tree';
import { TreeCursor } from './tree_cursor';
// eslint-disable-next-line @typescript-eslint/no-unused-vars
import { Language } from './language';
import { marshalNode, marshalPoint, unmarshalNode, unmarshalNodeArray, unmarshalPoint } from './marshal';
import { TRANSFER_BUFFER } from './parser';

/** A single node within a syntax {@link Tree}. */
export class Node {
  /** @internal */
  // @ts-expect-error: never read
  private [0] = 0; // Internal handle for Wasm

  /** @internal */
  private _children?: Node[];

  /** @internal */
  private _namedChildren?: Node[];

  /** @internal */
  constructor(
    internal: Internal,
    {
      id,
      tree,
      startIndex,
      startPosition,
      other,
    }: {
      id: number;
      tree: Tree;
      startIndex: number;
      startPosition: Point;
      other: number;
    }
  ) {
    assertInternal(internal);
    this[0] = other;
    this.id = id;
    this.tree = tree;
    this.startIndex = startIndex;
    this.startPosition = startPosition;
  }

  /**
   * The numeric id for this node that is unique.
   *
   * Within a given syntax tree, no two nodes have the same id.
   */
  id: number;

  /** The byte index where this node starts. */
  startIndex: number;

  /** The position where this node starts. */
  startPosition: Point;

  /** The tree that this node belongs to. */
  tree: Tree;

  /** Get this node's type as a numerical id. */
  get typeId(): number {
    marshalNode(this);
    return C._ts_node_symbol_wasm(this.tree[0]);
  }

  /**
   * Get the node's type as a numerical id as it appears in the grammar,
   * ignoring aliases.
   */
  get grammarId(): number {
    marshalNode(this);
    return C._ts_node_grammar_symbol_wasm(this.tree[0]);
  }

  /** Get this node's type as a string. */
  get type(): string {
    return this.tree.language.types[this.typeId] || 'ERROR';
  }

  /**
   * Get this node's symbol name as it appears in the grammar, ignoring
   * aliases as a string.
   */
  get grammarType(): string {
    return this.tree.language.types[this.grammarId] || 'ERROR';
  }

  /**
   * Check if this node is *named*.
   *
   * Named nodes correspond to named rules in the grammar, whereas
   * *anonymous* nodes correspond to string literals in the grammar.
   */
  get isNamed(): boolean {
    marshalNode(this);
    return C._ts_node_is_named_wasm(this.tree[0]) === 1;
  }

  /**
   * Check if this node is *extra*.
   *
   * Extra nodes represent things like comments, which are not required
   * by the grammar, but can appear anywhere.
   */
  get isExtra(): boolean {
    marshalNode(this);
    return C._ts_node_is_extra_wasm(this.tree[0]) === 1;
  }

  /**
   * Check if this node represents a syntax error.
   *
   * Syntax errors represent parts of the code that could not be incorporated
   * into a valid syntax tree.
   */
  get isError(): boolean {
    marshalNode(this);
    return C._ts_node_is_error_wasm(this.tree[0]) === 1;
  }

  /**
   * Check if this node is *missing*.
   *
   * Missing nodes are inserted by the parser in order to recover from
   * certain kinds of syntax errors.
   */
  get isMissing(): boolean {
    marshalNode(this);
    return C._ts_node_is_missing_wasm(this.tree[0]) === 1;
  }

  /**
   * Check if this node represents a syntax error or contains any syntax
   * errors anywhere within it.
   */
  get hasError(): boolean {
    marshalNode(this);
    return C._ts_node_has_error_wasm(this.tree[0]) === 1;
  }

  /** Get the byte index where this node ends. */
  get endIndex(): number {
    marshalNode(this);
    return C._ts_node_end_index_wasm(this.tree[0]);
  }

  /** Get the position where this node ends. */
  get endPosition(): Point {
    marshalNode(this);
    C._ts_node_end_point_wasm(this.tree[0]);
    return unmarshalPoint(TRANSFER_BUFFER);
  }

  /** Get the string content of this node. */
  get text(): string {
    const { textSource } = this.tree;
    return typeof textSource === 'string'
      ? textSource.slice(this.startIndex, this.endIndex)
      : getCallbackText(textSource, this.startIndex, this.endIndex, this.startPosition);
  }

  /** Get this node's parse state. */
  get parseState(): number {
    marshalNode(this);
    return C._ts_node_parse_state_wasm(this.tree[0]);
  }

  /** Get the parse state after this node. */
  get nextParseState(): number {
    marshalNode(this);
    return C._ts_node_next_parse_state_wasm(this.tree[0]);
  }

  /** Check if this node is equal to another node. */
  equals(other: Node): boolean {
    return this.tree === other.tree && this.id === other.id;
  }

  /**
   * Get the node's child at the given index, where zero represents the first child.
   *
   * This method is fairly fast, but its cost is technically log(n), so if
   * you might be iterating over a long list of children, you should use
   * {@link Node#children} instead.
   */
  child(index: number): Node | null {
    marshalNode(this);
    C._ts_node_child_wasm(this.tree[0], index);
    return unmarshalNode(this.tree);
  }

  /**
   * Get this node's *named* child at the given index.
   *
   * See also {@link Node#isNamed}.
   * This method is fairly fast, but its cost is technically log(n), so if
   * you might be iterating over a long list of children, you should use
   * {@link Node#namedChildren} instead.
   */
  namedChild(index: number): Node | null {
    marshalNode(this);
    C._ts_node_named_child_wasm(this.tree[0], index);
    return unmarshalNode(this.tree);
  }

  /**
   * Get this node's child with the given numerical field id.
   *
   * See also {@link Node#childForFieldName}. You can
   * convert a field name to an id using {@link Language#fieldIdForName}.
   */
  childForFieldId(fieldId: number): Node | null {
    marshalNode(this);
    C._ts_node_child_by_field_id_wasm(this.tree[0], fieldId);
    return unmarshalNode(this.tree);
  }

  /**
   * Get the first child with the given field name.
   *
   * If multiple children may have the same field name, access them using
   * {@link Node#childrenForFieldName}.
   */
  childForFieldName(fieldName: string): Node | null {
    const fieldId = this.tree.language.fields.indexOf(fieldName);
    if (fieldId !== -1) return this.childForFieldId(fieldId);
    return null;
  }

  /** Get the field name of this node's child at the given index. */
  fieldNameForChild(index: number): string | null {
    marshalNode(this);
    const address = C._ts_node_field_name_for_child_wasm(this.tree[0], index);
    if (!address) return null;
    return C.AsciiToString(address);
  }

  /** Get the field name of this node's named child at the given index. */
  fieldNameForNamedChild(index: number): string | null {
    marshalNode(this);
    const address = C._ts_node_field_name_for_named_child_wasm(this.tree[0], index);
    if (!address) return null;
    return C.AsciiToString(address);
  }
  /**
   * Get an array of this node's children with a given field name.
   *
   * See also {@link Node#children}.
   */
  childrenForFieldName(fieldName: string): Node[] {
    const fieldId = this.tree.language.fields.indexOf(fieldName);
    if (fieldId !== -1 && fieldId !== 0) return this.childrenForFieldId(fieldId);
    return [];
  }

  /**
    * Get an array of this node's children with a given field id.
    *
    * See also {@link Node#childrenForFieldName}.
    */
  childrenForFieldId(fieldId: number): Node[] {
    marshalNode(this);
    C._ts_node_children_by_field_id_wasm(this.tree[0], fieldId);
    return unmarshalNodeArray(this.tree, true);
  }

  /** Get the node's first child that contains or starts after the given byte offset. */
  firstChildForIndex(index: number): Node | null {
    marshalNode(this);
    const address = TRANSFER_BUFFER + SIZE_OF_NODE;
    C.HEAP32[address >>> 2] = index;
    C._ts_node_first_child_for_byte_wasm(this.tree[0]);
    return unmarshalNode(this.tree);
  }

  /** Get the node's first named child that contains or starts after the given byte offset. */
  firstNamedChildForIndex(index: number): Node | null {
    marshalNode(this);
    const address = TRANSFER_BUFFER + SIZE_OF_NODE;
    C.HEAP32[address >>> 2] = index;
    C._ts_node_first_named_child_for_byte_wasm(this.tree[0]);
    return unmarshalNode(this.tree);
  }

  /** Get this node's number of children. */
  get childCount(): number {
    marshalNode(this);
    return C._ts_node_child_count_wasm(this.tree[0]);
  }


  /**
   * Get this node's number of *named* children.
   *
   * See also {@link Node#isNamed}.
   */
  get namedChildCount(): number {
    marshalNode(this);
    return C._ts_node_named_child_count_wasm(this.tree[0]);
  }

  /** Get this node's first child. */
  get firstChild(): Node | null {
    return this.child(0);
  }

  /**
   * Get this node's first named child.
   *
   * See also {@link Node#isNamed}.
   */
  get firstNamedChild(): Node | null {
    return this.namedChild(0);
  }

  /** Get this node's last child. */
  get lastChild(): Node | null {
    return this.child(this.childCount - 1);
  }

  /**
   * Get this node's last named child.
   *
   * See also {@link Node#isNamed}.
   */
  get lastNamedChild(): Node | null {
    return this.namedChild(this.namedChildCount - 1);
  }

  /**
   * Iterate over this node's children.
   *
   * If you're walking the tree recursively, you may want to use the
   * {@link TreeCursor} APIs directly instead.
   */
  get children(): Node[] {
    if (!this._children) {
      marshalNode(this);
      C._ts_node_children_wasm(this.tree[0]);
      this._children = unmarshalNodeArray(this.tree);
    }
    return this._children;
  }

  /**
   * Iterate over this node's named children.
   *
   * See also {@link Node#children}.
   */
  get namedChildren(): Node[] {
    if (!this._namedChildren) {
      marshalNode(this);
      C._ts_node_named_children_wasm(this.tree[0]);
      this._namedChildren = unmarshalNodeArray(this.tree);
    }
    return this._namedChildren;
  }

  /**
   * Get the descendants of this node that are the given type, or in the given types array.
   *
   * The types array should contain node type strings, which can be retrieved from {@link Language#types}.
   *
   * Additionally, a `startPosition` and `endPosition` can be passed in to restrict the search to a byte range.
   */
  descendantsOfType(
    types: string | string[],
    startPosition: Point = ZERO_POINT,
    endPosition: Point = ZERO_POINT
  ): Node[] {
    if (!Array.isArray(types)) types = [types];

    // Convert the type strings to numeric type symbols
    const symbols: number[] = [];
    const typesBySymbol = this.tree.language.types;
    for (let i = 0, n = typesBySymbol.length; i < n; i++) {
      if (types.includes(typesBySymbol[i])) {
        symbols.push(i);
      }
    }
    // `symbols_contain` in tree-sitter.c expects ascending symbols, and
    // ts_builtin_sym_error is -1, which is UINT16_MAX, so it goes last.
    if (types.includes('ERROR')) {
      symbols.push(65535);
    }

    // Copy the array of symbols to the Wasm heap
    const symbolsAddress = C._malloc(SIZE_OF_INT * symbols.length);
    const symbolsHeap = C.HEAPU32;
    const symbolsBase = symbolsAddress >>> 2;
    for (let i = 0, n = symbols.length; i < n; i++) {
      symbolsHeap[symbolsBase + i] = symbols[i];
    }

    // Call the C API to compute the descendants
    marshalNode(this);
    const pointAddress = TRANSFER_BUFFER + SIZE_OF_NODE;
    marshalPoint(pointAddress, startPosition);
    marshalPoint(pointAddress + SIZE_OF_POINT, endPosition);
    C._ts_node_descendants_of_type_wasm(
      this.tree[0],
      symbolsAddress,
      symbols.length,
    );

    const result = unmarshalNodeArray(this.tree, true);
    C._free(symbolsAddress);
    return result;
  }

  /** Get this node's next sibling. */
  get nextSibling(): Node | null {
    marshalNode(this);
    C._ts_node_next_sibling_wasm(this.tree[0]);
    return unmarshalNode(this.tree);
  }

  /** Get this node's previous sibling. */
  get previousSibling(): Node | null {
    marshalNode(this);
    C._ts_node_prev_sibling_wasm(this.tree[0]);
    return unmarshalNode(this.tree);
  }

  /**
   * Get this node's next *named* sibling.
   *
   * See also {@link Node#isNamed}.
   */
  get nextNamedSibling(): Node | null {
    marshalNode(this);
    C._ts_node_next_named_sibling_wasm(this.tree[0]);
    return unmarshalNode(this.tree);
  }

  /**
   * Get this node's previous *named* sibling.
   *
   * See also {@link Node#isNamed}.
   */
  get previousNamedSibling(): Node | null {
    marshalNode(this);
    C._ts_node_prev_named_sibling_wasm(this.tree[0]);
    return unmarshalNode(this.tree);
  }

  /** Get the node's number of descendants, including one for the node itself. */
  get descendantCount(): number {
    marshalNode(this);
    return C._ts_node_descendant_count_wasm(this.tree[0]);
  }

  /**
   * Get this node's immediate parent.
   * Prefer {@link Node#childWithDescendant} for iterating over this node's ancestors.
   */
  get parent(): Node | null {
    marshalNode(this);
    C._ts_node_parent_wasm(this.tree[0]);
    return unmarshalNode(this.tree);
  }

  /**
   * Get the node that contains `descendant`.
   *
   * Note that this can return `descendant` itself.
   */
  childWithDescendant(descendant: Node): Node | null {
    if (descendant.tree !== this.tree) return null;
    marshalNode(this);
    marshalNode(descendant, 1);
    C._ts_node_child_with_descendant_wasm(this.tree[0]);
    return unmarshalNode(this.tree);
  }

  /** Get the smallest node within this node that spans the given byte range. */
  descendantForIndex(start: number, end: number = start): Node | null {
    if (typeof start !== 'number' || typeof end !== 'number') {
      throw new Error('Arguments must be numbers');
    }

    marshalNode(this);
    const base = (TRANSFER_BUFFER + SIZE_OF_NODE) >>> 2;
    const heap = C.HEAP32;
    heap[base] = start;
    heap[base + 1] = end;
    C._ts_node_descendant_for_index_wasm(this.tree[0]);
    return unmarshalNode(this.tree);
  }

  /** Get the smallest named node within this node that spans the given byte range. */
  namedDescendantForIndex(start: number, end: number = start): Node | null {
    if (typeof start !== 'number' || typeof end !== 'number') {
      throw new Error('Arguments must be numbers');
    }

    marshalNode(this);
    const base = (TRANSFER_BUFFER + SIZE_OF_NODE) >>> 2;
    const heap = C.HEAP32;
    heap[base] = start;
    heap[base + 1] = end;
    C._ts_node_named_descendant_for_index_wasm(this.tree[0]);
    return unmarshalNode(this.tree);
  }

  /** Get the smallest node within this node that spans the given point range. */
  descendantForPosition(start: Point, end: Point = start) {
    if (!isPoint(start) || !isPoint(end)) {
      throw new Error('Arguments must be {row, column} objects');
    }

    marshalNode(this);
    const address = TRANSFER_BUFFER + SIZE_OF_NODE;
    marshalPoint(address, start);
    marshalPoint(address + SIZE_OF_POINT, end);
    C._ts_node_descendant_for_position_wasm(this.tree[0]);
    return unmarshalNode(this.tree);
  }

  /** Get the smallest named node within this node that spans the given point range. */
  namedDescendantForPosition(start: Point, end: Point = start) {
    if (!isPoint(start) || !isPoint(end)) {
      throw new Error('Arguments must be {row, column} objects');
    }

    marshalNode(this);
    const address = TRANSFER_BUFFER + SIZE_OF_NODE;
    marshalPoint(address, start);
    marshalPoint(address + SIZE_OF_POINT, end);
    C._ts_node_named_descendant_for_position_wasm(this.tree[0]);
    return unmarshalNode(this.tree);
  }

  /**
   * Create a new {@link TreeCursor} starting from this node.
   *
   * Note that the given node is considered the root of the cursor,
   * and the cursor cannot walk outside this node.
   */
  walk(): TreeCursor {
    marshalNode(this);
    const address = C._ts_tree_cursor_new_wasm(this.tree[0]);
    return new TreeCursor(INTERNAL, this.tree, address);
  }

  /** Get the S-expression representation of this node. */
  toString(): string {
    marshalNode(this);
    const address = C._ts_node_to_string_wasm(this.tree[0]);
    const result = C.AsciiToString(address);
    C._ts_free_wasm(address);
    return result;
  }
}
