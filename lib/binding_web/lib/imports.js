mergeInto(LibraryManager.library, {
  tree_sitter_parse_callback(
    inputBufferAddress,
    maxCodeUnits,
    index,
    row,
    column,
    lengthAddress,
  ) {
    const source = Module.currentParseCallback;
    const isString = typeof source === 'string';
    const chunk = isString ? source : source(index, { row, column });
    const offset = isString ? index : 0;
    const remaining = typeof chunk === 'string' ? chunk.length - offset : 0;
    if (remaining <= 0) {
      HEAP32[lengthAddress >> 2] = 0;
      return;
    }
    const count = remaining < maxCodeUnits ? remaining : maxCodeUnits;
    const base = inputBufferAddress >>> 1;
    for (let i = 0; i < count; i++) {
      HEAPU16[base + i] = chunk.charCodeAt(offset + i);
    }
    HEAP32[lengthAddress >> 2] = count;
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
});
