/// <reference types="vite/client" />
import { expect, it, vi } from 'vitest';
import * as TreeSitter from '@willbooster/web-tree-sitter';
import playgroundHtml from '../../../../crates/cli/src/playground.html?raw';
import playgroundJsUrl from '../../../../crates/cli/src/playground.js?url';
import javascriptUrl from '../../../../target/release/tree-sitter-javascript.wasm?url';

interface PlaygroundTestParent extends Window {
  __playgroundErrors?: string[];
  __playgroundTreeSitter?: typeof TreeSitter;
}

interface CodeMirrorHostElement extends HTMLElement {
  CodeMirror?: {
    setValue(value: string): void;
  };
}

it('initializes the CLI playground and renders parsed trees without query controls', async () => {
  const languageBaseUrl = new URL('.', new URL(javascriptUrl, window.location.href)).href.replace(
    /\/$/,
    '',
  );
  const resolvedPlaygroundJsUrl = new URL(playgroundJsUrl, window.location.href).href;

  const parentWin = window as PlaygroundTestParent;
  parentWin.__playgroundErrors = [];
  parentWin.__playgroundTreeSitter = TreeSitter;

  const errorHook = `<script>
    window.addEventListener('error', (e) => parent.__playgroundErrors.push(String(e.error?.stack || e.message)));
    window.addEventListener('unhandledrejection', (e) => parent.__playgroundErrors.push(String(e.reason?.stack || e.reason)));
  </script>`;

  const html = playgroundHtml
    .replace('<head>', `<head>${errorHook}`)
    .replaceAll('THE_LANGUAGE_NAME', 'javascript')
    .replace('<option value="parser">', '<option value="javascript">')
    .replace('LANGUAGE_BASE_URL = ".";', `LANGUAGE_BASE_URL = ${JSON.stringify(languageBaseUrl)};`)
    .replace('src="playground.js"', `src=${JSON.stringify(resolvedPlaygroundJsUrl)}`)
    .replace(
      "import * as TreeSitter from './web-tree-sitter.js';",
      'const TreeSitter = parent.__playgroundTreeSitter;',
    );

  const pageUrl = URL.createObjectURL(new Blob([html], { type: 'text/html' }));
  const iframe = document.createElement('iframe');
  iframe.src = pageUrl;
  document.body.append(iframe);

  try {
    await vi.waitFor(
      () => {
        expect(parentWin.__playgroundErrors).toEqual([]);
        const container = iframe.contentDocument?.getElementById('playground-container');
        expect(container?.style.visibility).toBe('visible');
      },
      { timeout: 8000 },
    );

    const iframeDoc = iframe.contentDocument!;
    expect(iframeDoc.getElementById('query-input')).toBeNull();
    expect(iframeDoc.getElementById('query-checkbox')).toBeNull();
    expect(iframeDoc.getElementById('accessibility-checkbox')).toBeNull();

    const cmElement = iframeDoc.querySelector<CodeMirrorHostElement>('.CodeMirror');
    expect(cmElement?.CodeMirror).toBeDefined();
    cmElement!.CodeMirror!.setValue('let x = 1;');

    await vi.waitFor(() => {
      expect(parentWin.__playgroundErrors).toEqual([]);
      const treeOutput = iframeDoc.getElementById('output-container')?.textContent ?? '';
      expect(treeOutput).toContain('lexical_declaration');
      expect(treeOutput).toContain('variable_declarator');
      expect(treeOutput).toContain('number');
      expect(treeOutput).not.toContain('let');
    });

    const anonymousCheckbox = iframeDoc.getElementById(
      'anonymous-nodes-checkbox',
    ) as HTMLInputElement;
    anonymousCheckbox.click();

    await vi.waitFor(() => {
      expect(parentWin.__playgroundErrors).toEqual([]);
      const treeOutput = iframeDoc.getElementById('output-container')?.textContent ?? '';
      expect(treeOutput).toContain('let');
    });
  } finally {
    iframe.remove();
    URL.revokeObjectURL(pageUrl);
    delete parentWin.__playgroundErrors;
    delete parentWin.__playgroundTreeSitter;
  }
});
