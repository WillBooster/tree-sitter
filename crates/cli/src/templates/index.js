import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));

// Both standalone requires must stay relative so Bun embeds the addon and node-type metadata.
const isStandaloneBun = typeof Bun !== "undefined" && Bun.isStandaloneExecutable;
let binding;
if (isStandaloneBun) {
  // Catching the require lets Bun compile apps that bundle this grammar without loading it or supplying a prebuild.
  try {
    binding = require(`../../prebuilds/${process.platform}-${process.arch}/tree-sitter-KEBAB_PARSER_NAME.node`);
  } catch (error) {
    throw new Error(`Failed to load the grammar's native prebuild: ${String(error)}`, { cause: error });
  }
} else {
  binding = (await import("node-gyp-build")).default(root);
}

try {
  const nodeTypes = isStandaloneBun
    ? require("../../src/node-types.json")
    : (await import(`${root}/src/node-types.json`, { with: { type: "json" } })).default;
  binding.nodeTypeInfo = nodeTypes;
} catch { }

const queries = [
  ["HIGHLIGHTS_QUERY", `${root}/HIGHLIGHTS_QUERY_PATH`],
  ["INJECTIONS_QUERY", `${root}/INJECTIONS_QUERY_PATH`],
  ["LOCALS_QUERY", `${root}/LOCALS_QUERY_PATH`],
  ["TAGS_QUERY", `${root}/TAGS_QUERY_PATH`],
];

for (const [prop, path] of queries) {
  Object.defineProperty(binding, prop, {
    configurable: true,
    enumerable: true,
    get() {
      delete binding[prop];
      try {
        binding[prop] = readFileSync(path, "utf8");
      } catch { }
      return binding[prop];
    }
  });
}

export default binding;
