use super::{Error, metadata};
use std::{
    collections::{HashMap, VecDeque},
    ops,
    sync::atomic::AtomicUsize,
};
use tree_sitter::{
    CaptureQuantifier, Language, Node, Query, QueryCursor, QueryError, QueryMatch, Tree,
};

pub struct LocalDefinitionQuery {
    query: Query,
    context_capture: Option<u32>,
    value_capture: Option<u32>,
    definition_capture: Option<u32>,
}

type ExplicitValues = HashMap<(usize, usize), ops::Range<usize>>;

struct LocalDefinitionContext {
    range: ops::Range<usize>,
    value_range: ops::Range<usize>,
}

#[derive(Default)]
pub struct LocalDefinitions {
    pending: VecDeque<LocalDefinitionContext>,
    active: Vec<LocalDefinitionContext>,
    explicit_values: ExplicitValues,
}

impl LocalDefinitions {
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
        scope: &ops::Range<usize>,
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
            .filter(|context| {
                definition.end <= context.value_range.start
                    && scope.start <= context.range.start
                    && context.range.end <= scope.end
            })
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
        query: &LocalDefinitionQuery,
        cursor: &mut QueryCursor,
        tree: &Tree,
        source: &[u8],
        cancellation_flag: Option<&AtomicUsize>,
    ) -> Result<Self, Error> {
        let Some(value_capture) = query.value_capture else {
            return Ok(Self::default());
        };
        let mut contexts: HashMap<usize, LocalDefinitionContext> = HashMap::new();
        let mut explicit_values = HashMap::new();
        metadata::for_each_match(
            &query.query,
            cursor,
            tree,
            source,
            cancellation_flag,
            |mat| {
                let mut values = mat
                    .captures()
                    .iter()
                    .filter(|capture| capture.index == value_capture)
                    .map(|capture| capture.node.byte_range());
                let Some(mut value) = values.next() else {
                    return;
                };
                for next in values {
                    include_range(&mut value, &next);
                }
                if let Some(context) = mat
                    .captures()
                    .iter()
                    .find(|capture| Some(capture.index) == query.context_capture)
                    .map(|capture| capture.node)
                    && context.start_byte() <= value.start
                    && value.end <= context.end_byte()
                {
                    contexts
                        .entry(context.id())
                        .and_modify(|previous| include_range(&mut previous.value_range, &value))
                        .or_insert_with(|| LocalDefinitionContext {
                            range: context.byte_range(),
                            value_range: value.clone(),
                        });
                }
                query.record_explicit_values(mat, &mut explicit_values, value);
            },
        )?;
        let mut contexts = contexts.into_values().collect::<Vec<_>>();
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

impl LocalDefinitionQuery {
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
            && (main_query
                .capture_index_for_name("local.definition")
                .is_none()
                || main_query
                    .capture_index_for_name("local.definition-value")
                    .is_none())
        {
            return Ok(None);
        }
        Self::compile(language, locals_query)
            .map_err(|error| metadata::remap_error(error, locals_offset, query_source))
    }

    pub(super) fn disable_capture(&mut self, name: &str) {
        self.query.disable_capture(name);
        match name {
            "local.definition-value" => self.value_capture = None,
            "local.definition-context" => self.context_capture = None,
            _ => {}
        }
    }

    pub(super) fn disable_pattern(&mut self, index: usize) {
        self.query.disable_pattern(index);
    }

    fn record_explicit_values(
        &self,
        mat: &QueryMatch,
        values: &mut ExplicitValues,
        region: ops::Range<usize>,
    ) {
        for definition in mat
            .captures()
            .iter()
            .filter(|capture| Some(capture.index) == self.definition_capture)
        {
            values
                .entry((mat.pattern_index, definition.node.id()))
                .and_modify(|previous| include_range(previous, &region))
                .or_insert_with(|| region.clone());
        }
    }

    fn compile(language: &Language, locals_query: &str) -> Result<Option<Self>, QueryError> {
        let mut query = Query::new(language, locals_query)?;
        let context_capture = query.capture_index_for_name("local.definition-context");
        let value_capture = query.capture_index_for_name("local.definition-value");
        let definition_capture = query.capture_index_for_name("local.definition");
        let mut enabled = false;
        for pattern in 0..query.pattern_count() {
            let quantifiers = query.capture_quantifiers(pattern);
            if !context_capture
                .is_some_and(|context| quantifiers[context as usize] != CaptureQuantifier::Zero)
            {
                if !definition_capture.is_some_and(|definition| {
                    quantifiers[definition as usize] != CaptureQuantifier::Zero
                }) || !value_capture
                    .is_some_and(|value| quantifiers[value as usize] != CaptureQuantifier::Zero)
                {
                    query.disable_pattern(pattern);
                } else {
                    enabled = true;
                }
                continue;
            }
            if quantifiers[context_capture.unwrap() as usize] != CaptureQuantifier::One
                || value_capture.is_none_or(|value| {
                    !matches!(
                        quantifiers[value as usize],
                        CaptureQuantifier::One | CaptureQuantifier::OneOrMore
                    )
                })
            {
                return Err(metadata::structure_error(
                    locals_query,
                    query.start_byte_for_pattern(pattern),
                    "Each local.definition-context pattern must capture exactly one context and one or more local.definition-value nodes",
                ));
            }
            enabled = true;
        }
        if !enabled {
            return Ok(None);
        }
        Ok(Some(Self {
            query,
            context_capture,
            value_capture,
            definition_capture,
        }))
    }
}

fn include_range(region: &mut ops::Range<usize>, value: &ops::Range<usize>) {
    region.start = region.start.min(value.start);
    region.end = region.end.max(value.end);
}
