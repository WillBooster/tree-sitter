import { describe, it, expect, beforeAll, beforeEach, afterEach } from 'vitest';
import type { Point, Language, Tree, TreeCursor } from '../src';
import { Parser } from '../src';
import helper from './helper';

let JavaScript: Language;

interface CursorState {
  nodeType: string;
  nodeIsNamed: boolean;
  startPosition: Point;
  endPosition: Point;
  startIndex: number;
  endIndex: number;
}

describe('Tree', () => {
  let parser: Parser;
  let tree: Tree;

  beforeAll(async () => {
    ({ JavaScript } = await helper);
  });

  beforeEach(() => {
    parser = new Parser();
    parser.setLanguage(JavaScript);
  });

  afterEach(() => {
    parser.delete();
    tree.delete();
  });

  describe('.walk()', () => {
    let cursor: TreeCursor;

    afterEach(() => {
      cursor.delete();
    });

    it('returns a cursor that can be used to walk the tree', () => {
      tree = parser.parse('a * b + c / d')!;
      cursor = tree.walk();

      assertCursorState(cursor, {
        nodeType: 'program',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 0 },
        endPosition: { row: 0, column: 13 },
        startIndex: 0,
        endIndex: 13,
      });

      expect(cursor.gotoFirstChild()).toBe(true);
      assertCursorState(cursor, {
        nodeType: 'expression_statement',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 0 },
        endPosition: { row: 0, column: 13 },
        startIndex: 0,
        endIndex: 13,
      });

      expect(cursor.gotoFirstChild()).toBe(true);
      assertCursorState(cursor, {
        nodeType: 'binary_expression',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 0 },
        endPosition: { row: 0, column: 13 },
        startIndex: 0,
        endIndex: 13,
      });

      expect(cursor.gotoFirstChild()).toBe(true);
      assertCursorState(cursor, {
        nodeType: 'binary_expression',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 0 },
        endPosition: { row: 0, column: 5 },
        startIndex: 0,
        endIndex: 5,
      });

      expect(cursor.gotoFirstChild()).toBe(true);
      expect(cursor.nodeText).toBe('a');
      assertCursorState(cursor, {
        nodeType: 'identifier',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 0 },
        endPosition: { row: 0, column: 1 },
        startIndex: 0,
        endIndex: 1,
      });

      expect(cursor.gotoFirstChild()).toBe(false);
      expect(cursor.gotoNextSibling()).toBe(true);
      expect(cursor.nodeText).toBe('*');
      assertCursorState(cursor, {
        nodeType: '*',
        nodeIsNamed: false,
        startPosition: { row: 0, column: 2 },
        endPosition: { row: 0, column: 3 },
        startIndex: 2,
        endIndex: 3,
      });

      expect(cursor.gotoNextSibling()).toBe(true);
      expect(cursor.nodeText).toBe('b');
      assertCursorState(cursor, {
        nodeType: 'identifier',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 4 },
        endPosition: { row: 0, column: 5 },
        startIndex: 4,
        endIndex: 5,
      });

      expect(cursor.gotoNextSibling()).toBe(false);
      expect(cursor.gotoParent()).toBe(true);
      assertCursorState(cursor, {
        nodeType: 'binary_expression',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 0 },
        endPosition: { row: 0, column: 5 },
        startIndex: 0,
        endIndex: 5,
      });

      expect(cursor.gotoNextSibling()).toBe(true);
      assertCursorState(cursor, {
        nodeType: '+',
        nodeIsNamed: false,
        startPosition: { row: 0, column: 6 },
        endPosition: { row: 0, column: 7 },
        startIndex: 6,
        endIndex: 7,
      });

      expect(cursor.gotoNextSibling()).toBe(true);
      assertCursorState(cursor, {
        nodeType: 'binary_expression',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 8 },
        endPosition: { row: 0, column: 13 },
        startIndex: 8,
        endIndex: 13,
      });

      cursor.resetTo(cursor);
      assertCursorState(cursor, {
        nodeType: 'binary_expression',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 8 },
        endPosition: { row: 0, column: 13 },
        startIndex: 8,
        endIndex: 13,
      });

      const copy = tree.walk();
      copy.resetTo(cursor);

      expect(copy.gotoPreviousSibling()).toBe(true);
      assertCursorState(copy, {
        nodeType: '+',
        nodeIsNamed: false,
        startPosition: { row: 0, column: 6 },
        endPosition: { row: 0, column: 7 },
        startIndex: 6,
        endIndex: 7,
      });

      expect(copy.gotoPreviousSibling()).toBe(true);
      assertCursorState(copy, {
        nodeType: 'binary_expression',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 0 },
        endPosition: { row: 0, column: 5 },
        startIndex: 0,
        endIndex: 5,
      });

      expect(copy.gotoLastChild()).toBe(true);
      assertCursorState(copy, {
        nodeType: 'identifier',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 4 },
        endPosition: { row: 0, column: 5 },
        startIndex: 4,
        endIndex: 5,
      });

      expect(copy.gotoParent()).toBe(true);
      expect(copy.gotoParent()).toBe(true);
      expect(copy.nodeType).toBe('binary_expression');
      expect(copy.gotoParent()).toBe(true);
      expect(copy.nodeType).toBe('expression_statement');
      expect(copy.gotoParent()).toBe(true);
      expect(copy.nodeType).toBe('program');
      expect(copy.gotoParent()).toBe(false);
      copy.delete();

      expect(cursor.gotoParent()).toBe(true);
      expect(cursor.nodeType).toBe('binary_expression');
      expect(cursor.gotoParent()).toBe(true);
      expect(cursor.nodeType).toBe('expression_statement');
      expect(cursor.gotoParent()).toBe(true);
      expect(cursor.nodeType).toBe('program');
    });

    it('keeps track of the field name associated with each node', () => {
      tree = parser.parse('a.b();')!;
      cursor = tree.walk();
      cursor.gotoFirstChild();
      cursor.gotoFirstChild();

      expect(cursor.currentNode.type).toBe('call_expression');
      expect(cursor.currentFieldName).toBeNull();

      cursor.gotoFirstChild();
      expect(cursor.currentNode.type).toBe('member_expression');
      expect(cursor.currentFieldName).toBe('function');

      cursor.gotoFirstChild();
      expect(cursor.currentNode.type).toBe('identifier');
      expect(cursor.currentFieldName).toBe('object');

      cursor.gotoNextSibling();
      cursor.gotoNextSibling();
      expect(cursor.currentNode.type).toBe('property_identifier');
      expect(cursor.currentFieldName).toBe('property');

      cursor.gotoParent();
      cursor.gotoNextSibling();
      expect(cursor.currentNode.type).toBe('arguments');
      expect(cursor.currentFieldName).toBe('arguments');
    });

    it.each([false, true])('copies its own position after another cursor is used (deleted: %s)', (deleted) => {
      tree = parser.parse('a + b')!;
      cursor = tree.walk();
      cursor.gotoFirstChild();
      cursor.gotoFirstChild();
      cursor.gotoFirstChild();
      const expectedNode = cursor.currentNode;
      const expectedDepth = cursor.currentDepth;
      const other = tree.walk();
      expect(other.nodeType).toBe('program');
      if (deleted) other.delete();
      const copy = cursor.copy();
      try {
        expect(copy.currentNode.equals(expectedNode)).toBe(true);
        expect(copy.currentDepth).toBe(expectedDepth);
        expect(copy.gotoNextSibling()).toBe(true);
        expect(copy.nodeType).toBe('+');
        expect(cursor.currentNode.equals(expectedNode)).toBe(true);
      } finally {
        copy.delete();
        if (!deleted) other.delete();
      }
    });

    it('returns a cursor that can be reset anywhere in the tree', () => {
      tree = parser.parse('a * b + c / d')!;
      cursor = tree.walk();
      const root = tree.rootNode.firstChild;

      cursor.reset(root!.firstChild!.firstChild!);
      assertCursorState(cursor, {
        nodeType: 'binary_expression',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 0 },
        endPosition: { row: 0, column: 5 },
        startIndex: 0,
        endIndex: 5,
      });

      cursor.gotoFirstChild();
      assertCursorState(cursor, {
        nodeType: 'identifier',
        nodeIsNamed: true,
        startPosition: { row: 0, column: 0 },
        endPosition: { row: 0, column: 1 },
        startIndex: 0,
        endIndex: 1,
      });

      expect(cursor.gotoParent()).toBe(true);
      expect(cursor.gotoParent()).toBe(false);
    });

    it('returns a cursor that can be reset into another tree', () => {
      tree = parser.parse('a * b + c / d')!;
      const otherTree = parser.parse('xyz')!;
      cursor = tree.walk();

      cursor.reset(otherTree.rootNode.firstChild!);
      expect(cursor.nodeText).toBe('xyz');
      expect(cursor.currentNode.tree).toBe(otherTree);

      const original = tree.walk();
      cursor.resetTo(original);
      expect(cursor.nodeText).toBe('a * b + c / d');
      expect(cursor.currentNode.tree).toBe(tree);

      original.delete();
      otherTree.delete();
    });

    it('returns a copy that moves independently of the original', () => {
      tree = parser.parse('a * b + c / d')!;
      cursor = tree.walk();
      cursor.gotoFirstChild();
      cursor.gotoFirstChild();

      const copy = cursor.copy();
      copy.gotoFirstChild();
      cursor.gotoLastChild();
      expect(copy.nodeText).toBe('a * b');
      expect(cursor.nodeText).toBe('c / d');
      copy.delete();
    });

    it('returns a cursor that can move to the first child for an index', () => {
      tree = parser.parse('a * b + c / d')!;
      cursor = tree.walk();
      cursor.gotoFirstChild();
      cursor.gotoFirstChild();

      expect(cursor.gotoFirstChildForIndex(8)).toBe(true);
      expect(cursor.nodeText).toBe('c / d');

      cursor.gotoParent();
      expect(cursor.gotoFirstChildForIndex(0)).toBe(true);
      expect(cursor.nodeText).toBe('a * b');

      cursor.gotoParent();
      expect(cursor.gotoFirstChildForIndex(13)).toBe(false);
      expect(cursor.nodeText).toBe('a * b + c / d');
    });

    it('returns a cursor that can move to the first child for a position', () => {
      tree = parser.parse('a * b + c / d')!;
      cursor = tree.walk();
      cursor.gotoFirstChild();
      cursor.gotoFirstChild();

      expect(cursor.gotoFirstChildForPosition({ row: 0, column: 8 })).toBe(true);
      expect(cursor.nodeText).toBe('c / d');

      cursor.gotoParent();
      expect(cursor.gotoFirstChildForPosition({ row: 0, column: 0 })).toBe(true);
      expect(cursor.nodeText).toBe('a * b');

      cursor.gotoParent();
      expect(cursor.gotoFirstChildForPosition({ row: 1, column: 0 })).toBe(false);
      expect(cursor.nodeText).toBe('a * b + c / d');
    });

    it('preserves UTF-16 code unit slices, field names, and missing flags on cursor steps', () => {
      const source = 'const α = "🚀"; function f() { return 1;';
      tree = parser.parse(source)!;
      cursor = tree.walk();
      expect(cursor.gotoFirstChild()).toBe(true);
      expect(cursor.gotoFirstChild()).toBe(true);
      expect(cursor.gotoNextSibling()).toBe(true);
      expect(cursor.nodeType).toBe('variable_declarator');
      expect(cursor.gotoFirstChild()).toBe(true);
      expect(cursor.currentFieldName).toBe('name');
      expect(cursor.nodeText).toBe('α');
      expect(cursor.gotoNextSibling()).toBe(true);
      expect(cursor.gotoNextSibling()).toBe(true);
      expect(cursor.currentFieldName).toBe('value');
      expect(cursor.nodeText).toBe('"🚀"');
      expect(cursor.endIndex - cursor.startIndex).toBe('"🚀"'.length);

      let sawMissing = false;
      cursor.reset(tree.rootNode);
      for (;;) {
        if (cursor.nodeIsMissing) sawMissing = true;
        if (cursor.gotoFirstChild()) continue;
        while (!cursor.gotoNextSibling()) {
          if (!cursor.gotoParent()) {
            expect(sawMissing).toBe(true);
            return;
          }
        }
      }
    });

    it('preserves cursor position and parent stack when resetTo receives itself', () => {
      tree = parser.parse('const value = 1;')!;
      cursor = tree.walk();
      expect(cursor.gotoFirstChild()).toBe(true);
      expect(cursor.nodeType).toBe('lexical_declaration');
      expect(cursor.gotoFirstChild()).toBe(true);
      cursor.resetTo(cursor);
      expect(cursor.nodeText).toBe('const');
      expect(cursor.gotoParent()).toBe(true);
      expect(cursor.nodeType).toBe('lexical_declaration');
    });
  });

  describe('.copy', () => {
    it('creates an independent copy that outlives deleting the original tree', () => {
      const original = parser.parse('abc + cde')!;
      tree = original.copy();
      original.delete();

      expect(tree.rootNode.toString()).toBe(
        '(program (expression_statement (binary_expression left: (identifier) right: (identifier))))'
      );
      expect(tree.rootNode.firstChild!.firstChild!.firstChild!.text).toBe('abc');
      expect(tree.rootNode.firstChild!.firstChild!.lastChild!.text).toBe('cde');
    });
  });
});

function assertCursorState(cursor: TreeCursor, params: CursorState): void {
  expect(cursor.nodeType).toBe(params.nodeType);
  expect(cursor.nodeIsNamed).toBe(params.nodeIsNamed);
  expect(cursor.startPosition).toEqual(params.startPosition);
  expect(cursor.endPosition).toEqual(params.endPosition);
  expect(cursor.startIndex).toEqual(params.startIndex);
  expect(cursor.endIndex).toEqual(params.endIndex);

  const node = cursor.currentNode;
  expect(node.type).toBe(params.nodeType);
  expect(node.isNamed).toBe(params.nodeIsNamed);
  expect(node.startPosition).toEqual(params.startPosition);
  expect(node.endPosition).toEqual(params.endPosition);
  expect(node.startIndex).toEqual(params.startIndex);
  expect(node.endIndex).toEqual(params.endIndex);
}
