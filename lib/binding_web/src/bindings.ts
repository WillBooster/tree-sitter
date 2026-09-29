import createModule, { type MainModule } from '../lib/web-tree-sitter';
// eslint-disable-next-line @typescript-eslint/no-unused-vars
import { type Parser } from './parser';

export let Module: MainModule | null = null;

/** Options for {@link Parser.init}. */
export interface ParserInitOptions extends Partial<EmscriptenModule> {
  /**
   * The precompiled `web-tree-sitter.wasm` module. Pass it where compiling Wasm from bytes at run time is not
   * allowed, such as in Cloudflare Workers, which provide an imported `.wasm` file as a `WebAssembly.Module`.
   */
  wasmModule?: WebAssembly.Module;
}

/**
 * @internal
 *
 * Initialize the Tree-sitter Wasm module. This should only be called by the {@link Parser} class via {@link Parser.init}.
 */
export async function initializeBinding(options?: ParserInitOptions): Promise<MainModule> {
  if (Module) return Module;
  const { wasmModule, ...moduleOptions } = options ?? {};
  if (wasmModule) {
    // Emscripten reads the dynamic-linking metadata from the module passed with the instance, so both are passed.
    moduleOptions.instantiateWasm = (imports, receiveInstance) => {
      void WebAssembly.instantiate(wasmModule, imports).then((instance) => {
        (receiveInstance as (instance: WebAssembly.Instance, module: WebAssembly.Module) => void)(instance, wasmModule);
      });
      return {};
    };
  }
  return Module ??= await createModule(moduleOptions);
}

/**
 * @internal
 *
 * Checks if the Tree-sitter Wasm module has been initialized.
 */
export function checkModule(): boolean {
  return !!Module;
}
