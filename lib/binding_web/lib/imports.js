mergeInto(LibraryManager.library, {
  tree_sitter_install_scanner_allocators() {
    wasmImports.malloc = (size) => Module._ts_scanner_malloc_wasm(size);
    wasmImports.malloc.sig = 'ii';
    wasmImports.calloc = (count, size) => Module._ts_scanner_calloc_wasm(count, size);
    wasmImports.calloc.sig = 'iii';
    wasmImports.realloc = (pointer, size) => Module._ts_scanner_realloc_wasm(pointer, size);
    wasmImports.realloc.sig = 'iii';
  },

  tree_sitter_parse_callback(
    inputBufferAddress,
    index,
    row,
    column,
    lengthAddress,
  ) {
    const INPUT_BUFFER_SIZE = 10 * 1024;
    const string = Module.currentParseCallback(index, { row, column });
    if (typeof string === 'string') {
      setValue(lengthAddress, string.length, 'i32');
      stringToUTF16(string, inputBufferAddress, INPUT_BUFFER_SIZE);
    } else {
      setValue(lengthAddress, 0, 'i32');
    }
  },

  tree_sitter_log_callback(isLexMessage, messageAddress) {
    if (Module.currentLogCallback) {
      const message = UTF8ToString(messageAddress);
      Module.currentLogCallback(message, isLexMessage !== 0);
    }
  },

  tree_sitter_progress_callback(currentOffset, hasError) {
    if (Module.currentProgressCallback) {
      return Module.currentProgressCallback({ currentOffset, hasError: hasError !== 0 }) === true;
    }
    return false;
  },

  tree_sitter_query_progress_callback(currentOffset) {
    if (Module.currentQueryProgressCallback) {
      return Module.currentQueryProgressCallback({ currentOffset }) === true;
    }
    return false;
  },

  tree_sitter_set_memory_limit(maxBytes) {
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
  },

  tree_sitter_range_callback(index, address) {
    const range = Module.currentIncludedRanges[index];
    setValue(address, range.startPosition.row, 'i32');
    setValue(address + 4, range.startPosition.column, 'i32');
    setValue(address + 8, range.endPosition.row, 'i32');
    setValue(address + 12, range.endPosition.column, 'i32');
    setValue(address + 16, range.startIndex, 'i32');
    setValue(address + 20, range.endIndex, 'i32');
  },
});
