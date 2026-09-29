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
  // The declaration map gets an empty first line to stay aligned.
  await fs.writeFile(output, `/// <reference types="emscripten" />\n${await fs.readFile(output, 'utf8')}`);
  const map = JSON.parse(await fs.readFile(`${output}.map`, 'utf8'));
  map.mappings = `;${map.mappings}`;
  await fs.writeFile(`${output}.map`, JSON.stringify(map, null, '\t'));
}
