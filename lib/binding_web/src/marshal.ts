import { INTERNAL, Point, Range, SIZE_OF_NODE, SIZE_OF_RANGE, C } from "./constants";
import { Node } from "./node";
import { Tree } from "./tree";
import { TRANSFER_BUFFER } from "./parser";
import { LanguageMetadata } from "./language";
import { Edit } from "./edit";

/**
 * @internal
 *
 * Marshals a {@link Node} to the transfer buffer.
 */
export function marshalNode(node: Node, index = 0) {
  const heap = C.HEAP32;
  const idx = (TRANSFER_BUFFER + index * SIZE_OF_NODE) >> 2;
  heap[idx] = node.id;
  heap[idx + 1] = node.startIndex;
  heap[idx + 2] = node.startPosition.row;
  heap[idx + 3] = node.startPosition.column;
  heap[idx + 4] = node[0];
}

/**
 * @internal
 *
 * Unmarshals a heap-allocated array of {@link Node}s whose count and pointer sit in the transfer buffer.
 */
export function unmarshalNodes(tree: Tree, useTsFree = true): Node[] {
  const idx = TRANSFER_BUFFER >> 2;
  const count = C.HEAP32[idx];
  const buffer = C.HEAP32[idx + 1];
  const result = new Array<Node>(count);
  for (let i = 0, address = buffer; i < count; i++, address += SIZE_OF_NODE) {
    result[i] = unmarshalNode(tree, address)!;
  }
  if (buffer) {
    if (useTsFree) C._ts_free_wasm(buffer);
    else C._free(buffer);
  }
  return result;
}

/**
 * @internal
 *
 * Unmarshals a {@link Node} from the transfer buffer.
 */
export function unmarshalNode(tree: Tree, address = TRANSFER_BUFFER): Node | null {
  const heap = C.HEAP32;
  const idx = address >> 2;
  const id = heap[idx];
  if (id === 0) return null;

  return new Node(INTERNAL, {
    id,
    tree,
    startIndex: heap[idx + 1],
    startPosition: { row: heap[idx + 2], column: heap[idx + 3] },
    other: heap[idx + 4],
  });
}

/**
 * @internal
 *
 * Marshals a {@link Point} to the transfer buffer.
 */
export function marshalPoint(address: number, point: Point): void {
  const heap = C.HEAP32;
  const idx = address >> 2;
  heap[idx] = point.row;
  heap[idx + 1] = point.column;
}

/**
 * @internal
 *
 * Unmarshals a {@link Point} from the transfer buffer.
 */
export function unmarshalPoint(address: number): Point {
  const heap = C.HEAPU32;
  const idx = address >> 2;
  return {
    row: heap[idx],
    column: heap[idx + 1],
  };
}

/**
 * @internal
 *
 * Marshals a {@link Range} to the transfer buffer.
 */
export function marshalRange(address: number, range: Range): void {
  const heap = C.HEAP32;
  const idx = address >> 2;
  heap[idx] = range.startPosition.row;
  heap[idx + 1] = range.startPosition.column;
  heap[idx + 2] = range.endPosition.row;
  heap[idx + 3] = range.endPosition.column;
  heap[idx + 4] = range.startIndex;
  heap[idx + 5] = range.endIndex;
}

/**
 * @internal
 *
 * Unmarshals a heap-allocated array of {@link Range}s whose count and pointer sit in the transfer buffer.
 */
export function unmarshalRanges(useTsFree = true): Range[] {
  const idx = TRANSFER_BUFFER >> 2;
  const count = C.HEAP32[idx];
  const buffer = C.HEAP32[idx + 1];
  const result = new Array<Range>(count);
  for (let i = 0, address = buffer; i < count; i++, address += SIZE_OF_RANGE) {
    result[i] = unmarshalRange(address);
  }
  if (buffer) {
    if (useTsFree) C._ts_free_wasm(buffer);
    else C._free(buffer);
  }
  return result;
}

/**
 * @internal
 *
 * Unmarshals a {@link Range} from the transfer buffer.
 */
export function unmarshalRange(address: number): Range {
  const heap = C.HEAPU32;
  const idx = address >> 2;
  return {
    startPosition: { row: heap[idx], column: heap[idx + 1] },
    endPosition: { row: heap[idx + 2], column: heap[idx + 3] },
    startIndex: heap[idx + 4],
    endIndex: heap[idx + 5],
  };
}

/**
 * @internal
 *
 * Unmarshals a static `uint16_t` array whose count and pointer sit in the transfer buffer.
 */
export function unmarshalU16Array(): number[] {
  const idx = TRANSFER_BUFFER >> 2;
  const count = C.HEAP32[idx];
  const buffer = C.HEAP32[idx + 1];
  const result = new Array<number>(count);
  const heapU16 = C.HEAPU16;
  for (let i = 0, base = buffer >>> 1; i < count; i++) {
    result[i] = heapU16[base + i];
  }
  return result;
}

/**
 * @internal
 *
 * Marshals an {@link Edit} to the transfer buffer.
 */
export function marshalEdit(edit: Edit, address = TRANSFER_BUFFER) {
  const heap = C.HEAP32;
  const idx = address >> 2;
  heap[idx] = edit.startPosition.row;
  heap[idx + 1] = edit.startPosition.column;
  heap[idx + 2] = edit.oldEndPosition.row;
  heap[idx + 3] = edit.oldEndPosition.column;
  heap[idx + 4] = edit.newEndPosition.row;
  heap[idx + 5] = edit.newEndPosition.column;
  heap[idx + 6] = edit.startIndex;
  heap[idx + 7] = edit.oldEndIndex;
  heap[idx + 8] = edit.newEndIndex;
}

/**
 * @internal
 *
 * Unmarshals a {@link LanguageMetadata} from the transfer buffer.
 */
export function unmarshalLanguageMetadata(address: number): LanguageMetadata {
  const heap = C.HEAP32;
  const idx = address >> 2;
  return {
    major_version: heap[idx],
    minor_version: heap[idx + 1],
    patch_version: heap[idx + 2],
  };
}
