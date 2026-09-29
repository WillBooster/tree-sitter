import { createBundle } from 'dts-buddy';
import fs from 'fs/promises';

for (let ext of ['ts', 'cts']) {
  const output = `web-tree-sitter.d.${ext}`;
  await createBundle({
    project: 'tsconfig.json',
    output,
    modules: {
      '@willbooster/web-tree-sitter': 'src/index.ts'
    },
    compilerOptions: {
      stripInternal: true,
    },
  });
  // dts-buddy drops triple-slash references, and the public types extend `EmscriptenModule` from @types/emscripten.
  // The declaration map gets an empty first line to stay aligned. The `./debug` export shares these declarations.
  const declarations = (await fs.readFile(output, 'utf8')).replace(
    /^\/\/# sourceMappingURL=/m,
    `declare module '@willbooster/web-tree-sitter/debug' {\n\texport * from '@willbooster/web-tree-sitter';\n}\n\n$&`,
  );
  await fs.writeFile(output, `/// <reference types="emscripten" />\n${declarations}`);
  const map = JSON.parse(await fs.readFile(`${output}.map`, 'utf8'));
  map.mappings = `;${map.mappings}`;
  await fs.writeFile(`${output}.map`, JSON.stringify(map, null, '\t'));
}
