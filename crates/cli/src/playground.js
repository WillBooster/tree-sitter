window.initializePlayground = async (opts) => {
  const { Parser, Language } = window.TreeSitter;
  if (opts?.local) {
    initializeLocalTheme();
  }
  initializeCustomSelect({ addListeners: true });

  const elements = getPlaygroundElements();
  loadState(elements);

  await Parser.init();
  const parser = new Parser();
  const codeEditor = createCodeEditor(elements.codeInput);
  const cluster = new window.Clusterize({
    rows: [],
    noDataText: null,
    contentElem: elements.outputContainer,
    scrollElem: elements.outputContainerScroll,
  });

  const state = {
    parser,
    Language,
    codeEditor,
    cluster,
    elements,
    languagesByName: {},
    languageName: elements.languageSelect.value,
    tree: undefined,
    treeRows: undefined,
    treeRowHighlightedIndex: -1,
    parseCount: 0,
    isRendering: 0,
    renderTreeOnCodeChange: undefined,
    saveStateOnChange: undefined,
  };
  state.renderTreeOnCodeChange = debounce(() => renderTree(state), 50);
  state.saveStateOnChange = debounce(() => saveState(state), 2000);

  codeEditor.on('changes', () => handleCodeChange(state));
  codeEditor.on(
    'cursorActivity',
    debounce(() => handleCursorMovement(state), 150),
  );

  elements.loggingCheckbox.addEventListener('change', () => handleLoggingChange(state));
  elements.anonymousNodes.addEventListener('change', () => renderTree(state));
  elements.languageSelect.addEventListener('change', () => handleLanguageChange(state));
  elements.outputContainer.addEventListener('click', (event) => handleTreeClick(state, event));
  elements.copyButton?.addEventListener('click', () => handleCopy(elements.outputContainer));

  await handleLanguageChange(state);
  elements.playgroundContainer.style.visibility = 'visible';
};

function initializeLocalTheme() {
  const themeToggle = document.getElementById('theme-toggle');
  if (!themeToggle) return;

  const savedTheme = localStorage.getItem('theme');
  const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
  const initialTheme = savedTheme || (prefersDark ? 'dark' : 'light');
  document.documentElement.setAttribute('data-theme', initialTheme);

  themeToggle.addEventListener('click', () => {
    const currentTheme = document.documentElement.getAttribute('data-theme');
    const newTheme = currentTheme === 'light' ? 'dark' : 'light';
    document.documentElement.setAttribute('data-theme', newTheme);
    localStorage.setItem('theme', newTheme);
  });
}

function initializeCustomSelect({ initialValue, addListeners = false }) {
  const button = document.getElementById('language-button');
  const select = document.getElementById('language-select');
  if (!button || !select) return;

  const dropdown = button.nextElementSibling;
  const selectedValue = button.querySelector('.selected-value');

  if (initialValue) {
    select.value = initialValue;
  }
  if (select.selectedIndex >= 0 && select.options[select.selectedIndex]) {
    selectedValue.textContent = select.options[select.selectedIndex].text;
  } else {
    selectedValue.textContent = 'JavaScript';
  }

  if (!addListeners) return;

  button.addEventListener('click', (event) => {
    event.preventDefault();
    dropdown.classList.toggle('show');
  });

  document.addEventListener('click', (event) => {
    if (!button.contains(event.target)) {
      dropdown.classList.remove('show');
    }
  });

  for (const option of dropdown.querySelectorAll('.option')) {
    option.addEventListener('click', () => {
      selectedValue.textContent = option.textContent;
      select.value = option.dataset.value;
      dropdown.classList.remove('show');
      select.dispatchEvent(new Event('change'));
    });
  }
}

function getPlaygroundElements() {
  return {
    codeInput: document.getElementById('code-input'),
    languageSelect: document.getElementById('language-select'),
    languageVersion: document.getElementById('language-version'),
    loggingCheckbox: document.getElementById('logging-checkbox'),
    anonymousNodes: document.getElementById('anonymous-nodes-checkbox'),
    outputContainer: document.getElementById('output-container'),
    outputContainerScroll: document.getElementById('output-container-scroll'),
    playgroundContainer: document.getElementById('playground-container'),
    copyButton: document.getElementById('copy-button'),
    updateTimeSpan: document.getElementById('update-time'),
  };
}

function loadState(elements) {
  const language = localStorage.getItem('language');
  const sourceCode = localStorage.getItem('sourceCode');
  const anonNodes = localStorage.getItem('anonymousNodes');
  if (sourceCode !== null) {
    elements.codeInput.value = sourceCode;
  }
  if (anonNodes !== null) {
    elements.anonymousNodes.checked = anonNodes === 'true';
  }
  if (
    language !== null &&
    Array.from(elements.languageSelect.options).some((option) => option.value === language)
  ) {
    elements.languageSelect.value = language;
    initializeCustomSelect({ initialValue: language });
  }
}

function createCodeEditor(codeInput) {
  const editor = window.CodeMirror.fromTextArea(codeInput, {
    lineNumbers: true,
    showCursorWhenSelecting: true,
  });
  editor.on('keydown', (_, event) => {
    if (event.key === 'ArrowLeft' || event.key === 'ArrowRight' || event.key === '?') {
      event.stopPropagation();
    }
  });
  return editor;
}

async function handleLanguageChange(state) {
  const { elements, languagesByName, Language } = state;
  const newLanguageName = elements.languageSelect.value;
  if (!languagesByName[newLanguageName]) {
    const url = `${window.LANGUAGE_BASE_URL}/tree-sitter-${newLanguageName}.wasm`;
    elements.languageSelect.disabled = true;
    try {
      languagesByName[newLanguageName] = await Language.load(url);
    } catch (error) {
      console.error(error);
      elements.languageSelect.value = state.languageName;
      return;
    } finally {
      elements.languageSelect.disabled = false;
    }
  }

  state.tree?.delete();
  state.tree = undefined;
  state.languageName = newLanguageName;

  const metadata = languagesByName[newLanguageName].metadata;
  if (elements.languageVersion && metadata) {
    elements.languageVersion.textContent =
      `v${metadata.major_version}.${metadata.minor_version}.${metadata.patch_version}`;
    elements.languageVersion.style.visibility = 'visible';
  } else if (elements.languageVersion) {
    elements.languageVersion.style.visibility = 'hidden';
  }

  state.parser.setLanguage(languagesByName[newLanguageName]);
  handleCodeChange(state);
}

function handleCodeChange(state) {
  if (!state.parser.language) return;
  const newText = `${state.codeEditor.getValue()}\n`;
  const start = performance.now();
  const newTree = state.parser.parse(newText);
  const duration = (performance.now() - start).toFixed(1);

  state.elements.updateTimeSpan.innerText = `${duration} ms`;
  state.tree?.delete();
  state.tree = newTree;
  state.parseCount++;
  state.renderTreeOnCodeChange();
  state.saveStateOnChange();
}

async function renderTree(state) {
  if (!state.tree) return;
  state.isRendering++;
  const cursor = state.tree.walk();
  const currentRenderCount = state.parseCount;
  const walkState = {
    showAnonymous: state.elements.anonymousNodes.checked,
    rows: [],
    row: '',
    finishedRow: false,
    visitedChildren: false,
    indentLevel: 0,
  };

  for (let i = 0; ; i++) {
    if (i > 0 && i % 10000 === 0 && (await shouldAbortRender(state, currentRenderCount))) {
      cursor.delete();
      state.isRendering--;
      return;
    }
    if (!advanceRenderWalk(cursor, walkState)) break;
  }
  if (walkState.finishedRow) walkState.rows.push(`${walkState.row}</div>`);

  cursor.delete();
  state.cluster.update(walkState.rows);
  state.treeRows = walkState.rows;
  state.isRendering--;
  handleCursorMovement(state);
}

async function shouldAbortRender(state, currentRenderCount) {
  await new Promise((resolve) => setTimeout(resolve, 0));
  return state.parseCount !== currentRenderCount;
}

function advanceRenderWalk(cursor, walkState) {
  const displayName = cursorDisplayName(cursor, walkState.showAnonymous);
  if (walkState.visitedChildren) {
    if (displayName) walkState.finishedRow = true;
    return stepAfterChildren(cursor, walkState);
  }
  if (displayName) {
    if (walkState.finishedRow) walkState.rows.push(`${walkState.row}</div>`);
    walkState.row = formatTreeRow(cursor, displayName, walkState.indentLevel);
    walkState.finishedRow = true;
  }
  if (cursor.gotoFirstChild()) {
    walkState.visitedChildren = false;
    walkState.indentLevel++;
  } else {
    walkState.visitedChildren = true;
  }
  return true;
}

function cursorDisplayName(cursor, showAnonymous) {
  if (cursor.nodeIsMissing) {
    const nodeTypeText = cursor.nodeIsNamed ? cursor.nodeType : `"${cursor.nodeType}"`;
    return `MISSING ${nodeTypeText}`;
  }
  if (cursor.nodeIsNamed || showAnonymous) {
    return cursor.nodeType;
  }
  return undefined;
}

function stepAfterChildren(cursor, walkState) {
  if (cursor.gotoNextSibling()) {
    walkState.visitedChildren = false;
    return true;
  }
  if (cursor.gotoParent()) {
    walkState.visitedChildren = true;
    walkState.indentLevel--;
    return true;
  }
  return false;
}

function formatTreeRow(cursor, displayName, indentLevel) {
  const start = cursor.startPosition;
  const end = cursor.endPosition;
  const id = cursor.nodeId;
  const fieldPrefix = cursor.currentFieldName ? `${escapeHtml(cursor.currentFieldName)}: ` : '';
  const nodeClass =
    displayName === 'ERROR' || displayName.startsWith('MISSING')
      ? 'node-link error plain'
      : cursor.nodeIsNamed
        ? 'node-link named plain'
        : 'node-link anonymous plain';

  return (
    `<div class="tree-row">${'  '.repeat(indentLevel)}${fieldPrefix}` +
    `<a class='${nodeClass}' href="#" data-id=${id} ` +
    `data-range="${start.row},${start.column},${end.row},${end.column}">` +
    `${escapeHtml(displayName)}</a> <span class="position-info">` +
    `[${start.row}, ${start.column}] - [${end.row}, ${end.column}]</span>`
  );
}

function escapeHtml(value) {
  return value
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
}

function handleCursorMovement(state) {
  if (state.isRendering || !state.tree || !state.treeRows) return;

  const selection = state.codeEditor.getDoc().listSelections()[0];
  const [start, end] = orderedSelectionRange(selection);
  const node = state.tree.rootNode.namedDescendantForPosition(start, end);
  if (!node) return;

  const prevRow = state.treeRows[state.treeRowHighlightedIndex];
  if (prevRow) {
    state.treeRows[state.treeRowHighlightedIndex] = prevRow.replace('highlighted', 'plain');
  }
  state.treeRowHighlightedIndex = state.treeRows.findIndex((row) =>
    row.includes(`data-id=${node.id}`),
  );
  const nextRow = state.treeRows[state.treeRowHighlightedIndex];
  if (nextRow) {
    state.treeRows[state.treeRowHighlightedIndex] = nextRow.replace('plain', 'highlighted');
  }

  state.cluster.update(state.treeRows);
  scrollHighlightedRowIntoView(state);
}

function orderedSelectionRange(selection) {
  const start = { row: selection.anchor.line, column: selection.anchor.ch };
  const end = { row: selection.head.line, column: selection.head.ch };
  if (start.row > end.row || (start.row === end.row && start.column > end.column)) {
    return [end, start];
  }
  return [start, end];
}

function scrollHighlightedRowIntoView(state) {
  const scrollContainer = state.elements.outputContainerScroll;
  const lineHeight = state.cluster.options.item_height;
  const scrollTop = scrollContainer.scrollTop;
  const containerHeight = scrollContainer.clientHeight;
  const offset = state.treeRowHighlightedIndex * lineHeight;
  if (scrollTop > offset - 20) {
    scrollContainer.scrollTo({ top: offset - 20, behavior: 'smooth' });
  } else if (scrollTop < offset + lineHeight + 40 - containerHeight) {
    scrollContainer.scrollTo({ top: offset - containerHeight + 40, behavior: 'smooth' });
  }
}

function handleLoggingChange(state) {
  if (state.elements.loggingCheckbox.checked) {
    state.parser.setLogger((message, lexing) => {
      if (lexing) {
        console.log('  ', message);
      } else {
        console.log(message);
      }
    });
  } else {
    state.parser.setLogger(null);
  }
}

function handleTreeClick(state, event) {
  if (event.target.tagName !== 'A') return;
  event.preventDefault();
  const [startRow, startColumn, endRow, endColumn] = event.target.dataset.range
    .split(',')
    .map((value) => Number.parseInt(value, 10));
  state.codeEditor.focus();
  state.codeEditor.setSelection(
    { line: startRow, ch: startColumn },
    { line: endRow, ch: endColumn },
  );
}

function handleCopy(outputContainer) {
  const selection = window.getSelection();
  selection.removeAllRanges();
  const range = document.createRange();
  range.selectNodeContents(outputContainer);
  selection.addRange(range);
  navigator.clipboard.writeText(selection.toString());
  selection.removeRange(range);
  showToast('Tree copied to clipboard!');
}

function showToast(message) {
  document.querySelector('.toast')?.remove();
  const toast = document.createElement('div');
  toast.className = 'toast';
  toast.textContent = message;
  document.body.appendChild(toast);

  setTimeout(() => toast.classList.add('show'), 50);
  setTimeout(() => {
    toast.classList.remove('show');
    setTimeout(() => toast.remove(), 200);
  }, 1000);
}

function saveState(state) {
  localStorage.setItem('language', state.elements.languageSelect.value);
  localStorage.setItem('sourceCode', state.codeEditor.getValue());
  localStorage.setItem('anonymousNodes', state.elements.anonymousNodes.checked);
}

function debounce(func, wait) {
  let timeout;
  return function (...args) {
    clearTimeout(timeout);
    timeout = setTimeout(() => {
      timeout = undefined;
      func.apply(this, args);
    }, wait);
  };
}
