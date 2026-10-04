import './checkDslTypes.js';
import { createBundle } from 'dts-buddy';
import fs from 'fs/promises';

const moduleName = '@willbooster/web-tree-sitter';
const dslSource = new URL('../../../crates/generate/src/dsl.d.ts', import.meta.url);
await fs.copyFile(dslSource, 'dsl.d.ts');

for (let ext of ['ts', 'cts']) {
  const output = `web-tree-sitter.d.${ext}`;
  await createBundle({
    project: 'tsconfig.json',
    output,
    modules: {
      [moduleName]: 'src/index.ts',
    },
    compilerOptions: {
      stripInternal: true,
    },
  });
  // dts-buddy wraps the declarations in `declare module '<name>' { ... }`. A program that loads both the ES module and
  // the CommonJS declarations would then declare that module twice (TS6200), so each file becomes a module of its own,
  // which also serves the `./debug` export. The wrapper's first line becomes the triple-slash reference that dts-buddy
  // drops (the public types extend `EmscriptenModule` from @types/emscripten), which keeps the declaration map aligned.
  const header = `declare module '${moduleName}' {\n`;
  const footer = /^}\n(?=\n\/\/# sourceMappingURL=)/m;
  const declarations = await fs.readFile(output, 'utf8');
  if (!declarations.startsWith(header) || !footer.test(declarations)) {
    throw new Error(`Unexpected dts-buddy output in ${output}`);
  }
  // A private or protected member makes its class nominal, so the ES module and the CommonJS copies of the class
  // would not be assignable to each other in a program that passes values between both. Keep such members out of the
  // declarations.
  const nominalMember = /^\s*(?:private\b|protected\b|#private;).*$/m.exec(declarations);
  if (nominalMember) throw new Error(`Private or protected member in ${output}: ${nominalMember[0].trim()}`);
  await fs.writeFile(
    output,
    `/// <reference types="emscripten" />\n${declarations.slice(header.length).replace(footer, '')}`
  );
}
