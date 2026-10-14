import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import ts from 'typescript';

const [runtime, declarations] = await Promise.all([
  fs.readFile(new URL('../../../crates/generate/src/dsl.js', import.meta.url), 'utf8'),
  fs.readFile(new URL('../../../crates/generate/src/dsl.d.ts', import.meta.url), 'utf8'),
]);
const runtimeFile = ts.createSourceFile('dsl.js', runtime, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
const declarationFile = ts.createSourceFile('dsl.d.ts', declarations, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
const runtimeNames = runtimeFile.statements.flatMap((statement) => {
  if (!ts.isExpressionStatement(statement) || !ts.isBinaryExpression(statement.expression)) return [];
  const { left, operatorToken } = statement.expression;
  if (operatorToken.kind !== ts.SyntaxKind.EqualsToken || !ts.isPropertyAccessExpression(left)) return [];
  return ts.isIdentifier(left.expression) && left.expression.text === 'globalThis' ? [left.name.text] : [];
});
const dsl = declarationFile.statements.find((statement) =>
  ts.isInterfaceDeclaration(statement) && statement.name.text === 'DSL');
assert(dsl, 'The public DSL interface is missing.');
const declarationNames = dsl.members.map((member) => {
  assert(member.name && (ts.isIdentifier(member.name) || ts.isStringLiteral(member.name)),
    'DSL globals must have named declaration members.');
  return member.name.text;
});
assert.deepEqual([...new Set(runtimeNames)].sort(), [...new Set(declarationNames)].sort(),
  'The DSL declarations must match the globals injected by the CLI.');
