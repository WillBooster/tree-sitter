import { INTERNAL, Point, Range, SIZE_OF_NODE, SIZE_OF_RANGE, C } from './constants';
import { Node } from './node';
import { Tree } from './tree';
import { TRANSFER_BUFFER } from './parser';
import { LanguageMetadata } from './language';

/**
 * @internal
 *
 * Marshals a {@link Node} to the transfer buffer.
 */
export function marshalNode(node: Node, index = 0): void {
  const base = (TRANSFER_BUFFER + index * SIZE_OF_NODE) >>> 2;
  const heap = C.HEAP32;
  heap[base] = node.id;
  heap[base + 1] = node.startIndex;
  heap[base + 2] = node.startPosition.row;
  heap[base + 3] = node.startPosition.column;
  heap[base + 4] = node[0];
}

/**
 * @internal
 *
 * Unmarshals a {@link Node} from the transfer buffer.
 */
export function unmarshalNode(tree: Tree, address = TRANSFER_BUFFER): Node | null {
  const base = address >>> 2;
  const heap = C.HEAP32;
  const id = heap[base];
  if (id === 0) return null;

  const heapU32 = C.HEAPU32;
  return new Node(INTERNAL, {
    id,
    tree,
    startIndex: heapU32[base + 1],
    startPosition: { row: heapU32[base + 2], column: heapU32[base + 3] },
    other: heap[base + 4],
  });
}

/**
 * @internal
 *
 * Marshals a {@link Point} to the transfer buffer.
 */
export function marshalPoint(address: number, point: Point): void {
  const base = address >>> 2;
  const heap = C.HEAP32;
  heap[base] = point.row;
  heap[base + 1] = point.column;
}

/**
 * @internal
 *
 * Unmarshals a {@link Point} from the transfer buffer.
 */
export function unmarshalPoint(address: number): Point {
  const base = address >>> 2;
  const heap = C.HEAPU32;
  return {
    row: heap[base],
    column: heap[base + 1],
  };
}

/**
 * @internal
 *
 * Marshals a {@link Range} to the transfer buffer.
 */
export function marshalRange(address: number, range: Range): void {
  const base = address >>> 2;
  const heap = C.HEAP32;
  heap[base] = range.startPosition.row;
  heap[base + 1] = range.startPosition.column;
  heap[base + 2] = range.endPosition.row;
  heap[base + 3] = range.endPosition.column;
  heap[base + 4] = range.startIndex;
  heap[base + 5] = range.endIndex;
}

/**
 * @internal
 *
 * Unmarshals a {@link Range} from the transfer buffer.
 */
export function unmarshalRange(address: number): Range {
  const base = address >>> 2;
  const heap = C.HEAPU32;
  return {
    startPosition: { row: heap[base], column: heap[base + 1] },
    endPosition: { row: heap[base + 2], column: heap[base + 3] },
    startIndex: heap[base + 4],
    endIndex: heap[base + 5],
  };
}

/**
 * @internal
 *
 * Unmarshals a {@link LanguageMetadata} from the transfer buffer.
 */
export function unmarshalLanguageMetadata(address: number): LanguageMetadata {
  const base = address >>> 2;
  const heap = C.HEAP32;
  return {
    major_version: heap[base],
    minor_version: heap[base + 1],
    patch_version: heap[base + 2],
  };
}

/** @internal */
export function unmarshalSymbols(): number[] {
  const base = TRANSFER_BUFFER >>> 2;
  const count = C.HEAPU32[base];
  const buffer = C.HEAPU32[base + 1];
  const result = new Array<number>(count);
  if (count > 0) {
    const symbolBase = buffer >>> 1;
    const heapU16 = C.HEAPU16;
    for (let i = 0; i < count; i++) {
      result[i] = heapU16[symbolBase + i];
    }
  }
  return result;
}

/** @internal */
export function unmarshalRanges(useTsFree = true): Range[] {
  return unmarshalArray(SIZE_OF_RANGE, unmarshalRange, useTsFree);
}

/** @internal */
export function unmarshalNodeArray(tree: Tree, useTsFree = false): Node[] {
  return unmarshalArray(SIZE_OF_NODE, (address) => unmarshalNode(tree, address)!, useTsFree);
}

function unmarshalArray<T>(
  stride: number,
  readItem: (address: number) => T,
  useTsFree: boolean,
): T[] {
  const base = TRANSFER_BUFFER >>> 2;
  const count = C.HEAPU32[base];
  const buffer = C.HEAP32[base + 1];
  const result = new Array<T>(count);
  for (let i = 0, address = buffer; i < count; i++, address += stride) {
    result[i] = readItem(address);
  }
  if (buffer) {
    if (useTsFree) C._ts_free_wasm(buffer);
    else C._free(buffer);
  }
  return result;
}
