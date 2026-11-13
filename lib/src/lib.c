#include "./alloc.c"
#include "./language.c"
#include "./lexer.c"
#include "./node.c"
#include "./parser.c"
#include "./stack.c"
#include "./subtree.c"
#include "./tree_cursor.c"
#include "./tree.c"
#include "./wasm_store.c"

#ifdef TREE_SITTER_WASM_STDLIB
#include "./wasm-stdlib/libc.c"
#include "./wasm-stdlib/stdio.c"
#endif
