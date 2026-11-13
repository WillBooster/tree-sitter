import { INTERNAL, Internal, assertInternal, ParseCallback, Point, Range, SIZE_OF_NODE, SIZE_OF_INT, C } from './constants';
import { Language } from './language';
import { Node } from './node';
import { TreeCursor } from './tree_cursor';
import { marshalPoint, unmarshalNode, unmarshalRanges } from './marshal';
import { TRANSFER_BUFFER } from './parser';
import { newFinalizer } from './finalization_registry';

/** @internal */
export function getCallbackText(
  textSource: ParseCallback,
  startIndex: number,
  endIndex: number,
  startPosition: Point,
): string {
  const length = endIndex - startIndex;
  let result = textSource(startIndex, startPosition);
  if (result) {
    startIndex += result.length;
    while (startIndex < endIndex) {
      const string = textSource(startIndex, startPosition);
      if (string && string.length > 0) {
        startIndex += string.length;
        result += string;
      } else {
        break;
      }
    }
    if (startIndex > endIndex) {
      result = result.slice(0, length);
    }
  }
  return result ?? '';
}

const finalizer = newFinalizer((address: number) => {
  C._ts_tree_delete(address);
});

/** A tree that represents the syntactic structure of a source code file. */
export class Tree {
  /** @internal */
  private [0] = 0; // Internal handle for Wasm

  /** @internal */
  textSource: string | ParseCallback;

  /** The language that was used to parse the syntax tree. */
  language: Language;

  /** @internal */
  constructor(internal: Internal, address: number, language: Language, textSource: string | ParseCallback) {
    assertInternal(internal);
    this[0] = address;
    this.language = language;
    this.textSource = textSource;
    finalizer?.register(this, address, this);
  }

  /** Create a shallow copy of the syntax tree. This is very fast. */
  copy(): Tree {
    const address = C._ts_tree_copy(this[0]);
    return new Tree(INTERNAL, address, this.language, this.textSource);
  }

  /** Delete the syntax tree, freeing its resources. */
  delete(): void {
    finalizer?.unregister(this);
    C._ts_tree_delete(this[0]);
    this[0] = 0;
  }

  /** Get the root node of the syntax tree. */
  get rootNode(): Node {
    C._ts_tree_root_node_wasm(this[0]);
    return unmarshalNode(this)!;
  }

  /**
   * Get the root node of the syntax tree, but with its position shifted
   * forward by the given offset.
   */
  rootNodeWithOffset(offsetBytes: number, offsetExtent: Point): Node {
    const address = TRANSFER_BUFFER + SIZE_OF_NODE;
    C.HEAP32[address >>> 2] = offsetBytes;
    marshalPoint(address + SIZE_OF_INT, offsetExtent);
    C._ts_tree_root_node_with_offset_wasm(this[0]);
    return unmarshalNode(this)!;
  }

  /** Create a new {@link TreeCursor} starting from the root of the tree. */
  walk(): TreeCursor {
    C._ts_tree_root_node_wasm(this[0]);
    const address = C._ts_tree_cursor_new_wasm(this[0]);
    return new TreeCursor(INTERNAL, this, address);
  }

  /** Get the included ranges that were used to parse the syntax tree. */
  getIncludedRanges(): Range[] {
    C._ts_tree_included_ranges_wasm(this[0]);
    return unmarshalRanges();
  }
}
