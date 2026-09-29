import esbuild from 'esbuild';
import fs from 'fs/promises';

const format = process.env.CJS ? 'cjs' : 'esm';
const debug = process.argv.includes('--debug');
const outfile = `${debug ? 'debug/' : ''}web-tree-sitter.${format === 'esm' ? 'js' : 'cjs'}`;

async function processWasmSourceMap(inputPath, outputPath) {
  const mapContent = await fs.readFile(inputPath, 'utf8');
  const sourceMap = JSON.parse(mapContent);

  const isTreeSitterSource = (source) => 
    source.includes('../../src/') || source === 'tree-sitter.c';

  const normalizePath = (source) => {
    if (source.includes('../../src/')) {
      return source.replace('../../src/', debug ? '../lib/' : 'lib/');
    } else if (source === 'tree-sitter.c') {
      return debug ? '../lib/tree-sitter.c' : 'lib/tree-sitter.c';
    }
    return source;
  };

  const filtered = sourceMap.sources
    .map((source, index) => ({ source, content: sourceMap.sourcesContent?.[index] }))
    .filter(item => isTreeSitterSource(item.source))
    .map(item => ({ source: normalizePath(item.source), content: item.content }));

  sourceMap.sources = filtered.map(item => item.source);
  sourceMap.sourcesContent = filtered.map(item => item.content);

  await fs.writeFile(outputPath, JSON.stringify(sourceMap, null, 2));
}

const withoutNodeBuiltins = {
  name: 'without-node-builtins',
  setup(build) {
    build.onResolve({ filter: /^(fs\/promises|module)$/ }, (args) => ({ path: args.path, namespace: 'node-builtin' }));
    build.onLoad({ filter: /.*/, namespace: 'node-builtin' }, (args) => ({
      contents: `const unavailable = () => { throw new Error(${JSON.stringify(`${args.path} is not available here`)}); };
export const createRequire = unavailable;
export const readFile = unavailable;
export default {};`,
      loader: 'js',
    }));
  },
};

async function build() {
  await esbuild.build({
    entryPoints: ['src/index.ts'],
    bundle: true,
    platform: 'node',
    format,
    outfile,
    sourcemap: true,
    sourcesContent: true,
    keepNames: true,
    external: ['fs/*', 'fs/promises'],
    resolveExtensions: ['.ts', '.js', format === 'esm' ? '.mjs' : '.cjs'],
    ...(format === 'cjs' ? {
      footer: { js: 'module.exports.default = module.exports;' },
    } : {}),
  });

  if (format === 'esm' && !debug) {
    // Browsers and Cloudflare Workers get a release bundle without the Node.js-only code paths, whose imports of
    // Node.js built-in modules their bundlers cannot resolve. Workers with Node.js compatibility define `process`, so
    // it is replaced to keep them from taking those paths.
    await esbuild.build({
      entryPoints: ['src/index.ts'],
      bundle: true,
      platform: 'neutral',
      format,
      outfile: outfile.replace(/\.js$/, '.web.js'),
      sourcemap: true,
      sourcesContent: true,
      keepNames: true,
      resolveExtensions: ['.ts', '.js', '.mjs'],
      define: { process: 'undefined', 'globalThis.process': 'undefined' },
      plugins: [withoutNodeBuiltins],
    });
  }

  // Copy the Wasm files to the appropriate spot, as esbuild doesn't "bundle" Wasm files
  const outputWasmName = `${debug ? 'debug/' : ''}web-tree-sitter.wasm`;
  await fs.copyFile('lib/web-tree-sitter.wasm', outputWasmName);

  await processWasmSourceMap('lib/web-tree-sitter.wasm.map', `${outputWasmName}.map`);
}

build().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
