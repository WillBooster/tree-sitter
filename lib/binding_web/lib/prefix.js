Module.currentQueryProgressCallback = null;
Module.currentProgressCallback = null;
Module.currentLogCallback = null;
Module.currentParseCallback = null;
Module.treeSitterSetMemoryLimit = function (maxBytes) {
  if (!maxBytes) {
    if (Module.treeSitterOriginalMemoryGrow) {
      wasmMemory.grow = Module.treeSitterOriginalMemoryGrow;
      Module.treeSitterOriginalMemoryGrow = null;
    }
    return true;
  }
  if (wasmMemory.buffer.byteLength > maxBytes) return false;
  if (Module.treeSitterOriginalMemoryGrow) return false;
  Module.treeSitterOriginalMemoryGrow = wasmMemory.grow;
  wasmMemory.grow = function (pages) {
    if (wasmMemory.buffer.byteLength + pages * 65536 > maxBytes) {
      throw new RangeError('Parse memory limit exceeded');
    }
    return Module.treeSitterOriginalMemoryGrow.call(wasmMemory, pages);
  };
  return true;
};
