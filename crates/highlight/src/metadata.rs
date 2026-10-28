use super::Error;
use std::{
    ops::ControlFlow,
    sync::atomic::{AtomicUsize, Ordering},
};
use streaming_iterator::StreamingIterator;
use tree_sitter::{
    Query, QueryCursor, QueryCursorOptions, QueryCursorState, QueryError, QueryErrorKind,
    QueryMatch, Tree,
};

pub fn for_each_match(
    query: &Query,
    cursor: &mut QueryCursor,
    tree: &Tree,
    source: &[u8],
    cancellation_flag: Option<&AtomicUsize>,
    mut collect: impl FnMut(&QueryMatch),
) -> Result<(), Error> {
    let mut progress = |_: &QueryCursorState| {
        if is_cancelled(cancellation_flag) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = QueryCursorOptions::new().progress_callback(&mut progress);
    let mut matches = cursor.matches_with_options(query, tree.root_node(), source, options);
    while let Some(mat) = matches.next() {
        if is_cancelled(cancellation_flag) {
            return Err(Error::Cancelled);
        }
        collect(mat);
    }
    if is_cancelled(cancellation_flag) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

pub fn structure_error(source: &str, offset: usize, message: &str) -> QueryError {
    remap_error(
        QueryError {
            row: 0,
            column: 0,
            offset,
            message: message.into(),
            kind: QueryErrorKind::Structure,
        },
        0,
        source,
    )
}

pub fn remap_error(mut error: QueryError, offset: usize, source: &str) -> QueryError {
    error.offset += offset;
    let prefix = &source[..error.offset];
    error.row = prefix.bytes().filter(|byte| *byte == b'\n').count();
    error.column = prefix.rsplit('\n').next().unwrap().len();
    error
}

fn is_cancelled(flag: Option<&AtomicUsize>) -> bool {
    flag.is_some_and(|flag| flag.load(Ordering::Relaxed) != 0)
}
