mergeInto(LibraryManager.library, {
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
      HEAP32[lengthAddress >> 2] = string.length;
      stringToUTF16(string, inputBufferAddress, INPUT_BUFFER_SIZE);
    } else {
      HEAP32[lengthAddress >> 2] = 0;
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
});
