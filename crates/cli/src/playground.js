const input = document.getElementById('code-input');
const output = document.getElementById('output-container');
const status = document.getElementById('status');

try {
  const { Language, Parser } = await import('./web-tree-sitter.js');
  await Parser.init();
  const language = await Language.load('./tree-sitter-parser.wasm');
  const parser = new Parser();
  parser.setLanguage(language);
  const parse = () => {
    const tree = parser.parse(input.value);
    output.textContent = tree.rootNode.toString();
    tree.delete();
  };
  input.addEventListener('input', parse);
  parse();
  status.textContent = '';
} catch (error) {
  status.textContent = `Failed to load parser: ${error.message}`;
}
