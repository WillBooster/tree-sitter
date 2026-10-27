use super::Error;
use std::{
    collections::{HashMap, VecDeque},
    ops::{self, ControlFlow},
    sync::atomic::{AtomicUsize, Ordering},
};
use streaming_iterator::StreamingIterator;
use tree_sitter::{
    CaptureQuantifier, Language, Node, Query, QueryCursor, QueryCursorOptions, QueryCursorState,
    QueryError, QueryErrorKind, Tree,
};

pub struct LocalDefinitionContextQuery {
    query: Query,
    context_capture: u32,
    value_capture: u32,
    definition_capture: Option<u32>,
}

struct LocalDefinitionContext {
    range: ops::Range<usize>,
    value_range: ops::Range<usize>,
}

#[derive(Default)]
pub struct LocalDefinitionContexts {
    pending: VecDeque<LocalDefinitionContext>,
    active: Vec<LocalDefinitionContext>,
    explicit_values: HashMap<(usize, usize), ops::Range<usize>>,
}

impl LocalDefinitionContexts {
    pub(super) fn explicit_value_range(
        &self,
        pattern: usize,
        node: Node,
    ) -> Option<ops::Range<usize>> {
        self.explicit_values.get(&(pattern, node.id())).cloned()
    }

    pub(super) fn value_range(
        &mut self,
        definition: &ops::Range<usize>,
    ) -> Option<ops::Range<usize>> {
        while self
            .pending
            .front()
            .is_some_and(|context| context.range.start <= definition.start)
        {
            self.active.push(self.pending.pop_front().unwrap());
        }
        self.active
            .retain(|context| context.value_range.start >= definition.start);
        self.active
            .iter()
            .filter(|context| definition.end <= context.value_range.start)
            .min_by_key(|context| {
                (
                    context.range.len(),
                    context.range.start,
                    context.value_range.start,
                    context.value_range.end,
                )
            })
            .map(|context| context.value_range.clone())
    }

    pub(super) fn collect(
        query: &LocalDefinitionContextQuery,
        cursor: &mut QueryCursor,
        tree: &Tree,
        source: &[u8],
        cancellation_flag: Option<&AtomicUsize>,
    ) -> Result<Self, Error> {
        let mut contexts = Vec::new();
        let mut explicit_values = HashMap::new();
        let mut progress = |_: &QueryCursorState| {
            if cancellation_flag.is_some_and(|flag| flag.load(Ordering::Relaxed) != 0) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let options = QueryCursorOptions::new().progress_callback(&mut progress);
        let mut matches =
            cursor.matches_with_options(&query.query, tree.root_node(), source, options);
        while let Some(mat) = matches.next() {
            if cancellation_flag.is_some_and(|flag| flag.load(Ordering::Relaxed) != 0) {
                return Err(Error::Cancelled);
            }
            let Some(value) = mat
                .captures()
                .iter()
                .rev()
                .find(|capture| capture.index == query.value_capture)
                .map(|capture| capture.node.byte_range())
            else {
                continue;
            };
            if let Some(context) = mat
                .captures()
                .iter()
                .find(|capture| capture.index == query.context_capture)
                .map(|capture| capture.node.byte_range())
                && context.start <= value.start
                && value.end <= context.end
            {
                contexts.push(LocalDefinitionContext {
                    range: context,
                    value_range: value.clone(),
                });
            }
            for definition in mat
                .captures()
                .iter()
                .filter(|capture| Some(capture.index) == query.definition_capture)
            {
                explicit_values.insert((mat.pattern_index, definition.node.id()), value.clone());
            }
        }
        if cancellation_flag.is_some_and(|flag| flag.load(Ordering::Relaxed) != 0) {
            return Err(Error::Cancelled);
        }
        contexts.sort_unstable_by_key(|context| {
            (
                context.range.start,
                context.range.end,
                context.value_range.start,
                context.value_range.end,
            )
        });
        contexts.dedup_by(|a, b| a.range == b.range && a.value_range == b.value_range);
        Ok(Self {
            pending: contexts.into(),
            active: Vec::new(),
            explicit_values,
        })
    }
}

impl LocalDefinitionContextQuery {
    pub(super) fn new(
        language: &Language,
        locals_query: &str,
        main_query: &Query,
        locals_offset: usize,
        query_source: &str,
    ) -> Result<Option<Self>, QueryError> {
        if main_query
            .capture_index_for_name("local.definition-context")
            .is_none()
        {
            return Ok(None);
        }
        Self::compile(language, locals_query).map_err(|mut error| {
            error.offset += locals_offset;
            let prefix = &query_source[..error.offset];
            error.row = prefix.bytes().filter(|byte| *byte == b'\n').count();
            error.column = prefix.rsplit('\n').next().unwrap().len();
            error
        })
    }

    fn compile(language: &Language, locals_query: &str) -> Result<Option<Self>, QueryError> {
        let mut query = Query::new(language, locals_query)?;
        let Some(context_capture) = query.capture_index_for_name("local.definition-context") else {
            return Ok(None);
        };
        let value_capture = query.capture_index_for_name("local.definition-value");
        let definition_capture = query.capture_index_for_name("local.definition");
        for pattern in 0..query.pattern_count() {
            let quantifiers = query.capture_quantifiers(pattern);
            if quantifiers[context_capture as usize] == CaptureQuantifier::Zero {
                if !definition_capture.is_some_and(|definition| {
                    quantifiers[definition as usize] != CaptureQuantifier::Zero
                }) || !value_capture
                    .is_some_and(|value| quantifiers[value as usize] != CaptureQuantifier::Zero)
                {
                    query.disable_pattern(pattern);
                }
                continue;
            }
            if quantifiers[context_capture as usize] != CaptureQuantifier::One
                || value_capture
                    .is_none_or(|value| quantifiers[value as usize] != CaptureQuantifier::One)
            {
                let offset = query.start_byte_for_pattern(pattern);
                let prefix = &locals_query[..offset];
                return Err(QueryError {
                    row: prefix.bytes().filter(|b| *b == b'\n').count(),
                    column: prefix.rsplit('\n').next().unwrap().len(),
                    offset,
                    message: "Each local.definition-context pattern must capture exactly one context and one local.definition-value".into(),
                    kind: QueryErrorKind::Structure,
                });
            }
        }
        Ok(Some(Self {
            query,
            context_capture,
            value_capture: value_capture.unwrap(),
            definition_capture,
        }))
    }
}
