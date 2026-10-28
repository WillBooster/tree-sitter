use super::{Error, LocalDef, LocalScope};
use std::{
    collections::HashMap,
    ops::ControlFlow,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};
use streaming_iterator::StreamingIterator;
use tree_sitter::{
    CaptureQuantifier, Language, Node, Query, QueryCursor, QueryCursorOptions, QueryCursorState,
    QueryError, QueryErrorKind, Tree,
};

pub struct ScopeLinkQuery {
    query: Query,
    source_capture: Option<u32>,
    import_capture: Option<u32>,
    scope_enabled: bool,
}

struct ImportedBindings<'a> {
    frames: usize,
    definitions: Rc<Vec<LocalDef<'a>>>,
}

pub struct ScopeLinks<'a> {
    imports: HashMap<usize, usize>,
    remaining_imports: HashMap<usize, usize>,
    snapshots: HashMap<usize, Rc<Vec<LocalDef<'a>>>>,
    active_imports: HashMap<usize, ImportedBindings<'a>>,
    scope_nodes: Vec<usize>,
}

impl ScopeLinkQuery {
    pub(super) fn new(
        language: &Language,
        locals_query: &str,
        main_query: &Query,
        locals_offset: usize,
        query_source: &str,
    ) -> Result<Option<Self>, QueryError> {
        if main_query
            .capture_index_for_name("local.scope-source")
            .is_none()
            && main_query
                .capture_index_for_name("local.scope-import")
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

    pub(super) fn disable_capture(&mut self, name: &str) {
        self.query.disable_capture(name);
        match name {
            "local.scope-source" => self.source_capture = None,
            "local.scope-import" => self.import_capture = None,
            "local.scope" => self.scope_enabled = false,
            _ => {}
        }
    }

    pub(super) fn disable_pattern(&mut self, index: usize) {
        self.query.disable_pattern(index);
    }

    pub(super) fn collect<'a>(
        &self,
        cursor: &mut QueryCursor,
        tree: &Tree,
        source: &[u8],
        cancellation_flag: Option<&AtomicUsize>,
    ) -> Result<Option<ScopeLinks<'a>>, Error> {
        let (Some(source_capture), Some(import_capture), true) =
            (self.source_capture, self.import_capture, self.scope_enabled)
        else {
            return Ok(None);
        };
        let mut imports = HashMap::new();
        let mut progress = |_: &QueryCursorState| {
            if cancellation_flag.is_some_and(|flag| flag.load(Ordering::Relaxed) != 0) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let options = QueryCursorOptions::new().progress_callback(&mut progress);
        let mut matches =
            cursor.matches_with_options(&self.query, tree.root_node(), source, options);
        while let Some(mat) = matches.next() {
            if cancellation_flag.is_some_and(|flag| flag.load(Ordering::Relaxed) != 0) {
                return Err(Error::Cancelled);
            }
            let Some(origin) = mat.captures().iter().find(|c| c.index == source_capture) else {
                continue;
            };
            let source_id = origin.node.id();
            let source_end = origin.node.end_byte();
            for target in mat.captures().iter().filter(|c| c.index == import_capture) {
                let target_id = target.node.id();
                if source_id == target_id || source_end > target.node.start_byte() {
                    continue;
                }
                imports
                    .entry(target_id)
                    .and_modify(|previous: &mut Option<usize>| {
                        *previous = previous.filter(|id| *id == source_id);
                    })
                    .or_insert_with(|| Some(source_id));
            }
        }
        if cancellation_flag.is_some_and(|flag| flag.load(Ordering::Relaxed) != 0) {
            return Err(Error::Cancelled);
        }
        Ok(ScopeLinks::from_associations(imports))
    }

    fn compile(language: &Language, locals_query: &str) -> Result<Option<Self>, QueryError> {
        let mut query = Query::new(language, locals_query)?;
        let source_capture = query.capture_index_for_name("local.scope-source");
        let import_capture = query.capture_index_for_name("local.scope-import");
        if source_capture.is_none() && import_capture.is_none() {
            return Ok(None);
        }
        let scope_enabled = query.capture_index_for_name("local.scope").is_some();
        for pattern in 0..query.pattern_count() {
            let quantifiers = query.capture_quantifiers(pattern);
            let source =
                source_capture.map_or(CaptureQuantifier::Zero, |i| quantifiers[i as usize]);
            let import =
                import_capture.map_or(CaptureQuantifier::Zero, |i| quantifiers[i as usize]);
            if source == CaptureQuantifier::Zero && import == CaptureQuantifier::Zero {
                query.disable_pattern(pattern);
            } else if !scope_enabled
                || source != CaptureQuantifier::One
                || !matches!(
                    import,
                    CaptureQuantifier::One | CaptureQuantifier::OneOrMore
                )
            {
                let offset = query.start_byte_for_pattern(pattern);
                let prefix = &locals_query[..offset];
                return Err(QueryError {
                    row: prefix.bytes().filter(|b| *b == b'\n').count(),
                    column: prefix.rsplit('\n').next().unwrap().len(),
                    offset,
                    message: "Each scope-link pattern requires exactly one local.scope-source and one or more local.scope-import nodes; local.scope captures must also be configured".into(),
                    kind: QueryErrorKind::Structure,
                });
            }
        }
        Ok(Some(Self {
            query,
            source_capture,
            import_capture,
            scope_enabled,
        }))
    }
}

impl<'a> ScopeLinks<'a> {
    fn from_associations(associations: HashMap<usize, Option<usize>>) -> Option<Self> {
        let imports = associations
            .into_iter()
            .filter_map(|(target, origin)| origin.map(|id| (target, id)))
            .collect::<HashMap<_, _>>();
        if imports.is_empty() {
            return None;
        }
        let mut remaining_imports = HashMap::new();
        for origin in imports.values() {
            *remaining_imports.entry(*origin).or_insert(0) += 1;
        }
        Some(Self {
            imports,
            remaining_imports,
            snapshots: HashMap::new(),
            active_imports: HashMap::new(),
            scope_nodes: Vec::new(),
        })
    }

    pub(super) fn enter(&mut self, node: Node, depth: usize) {
        debug_assert_eq!(depth, self.scope_nodes.len() + 1);
        let id = node.id();
        self.scope_nodes.push(id);
        if let Some(imported) = self.active_imports.get_mut(&id) {
            imported.frames += 1;
        } else if let Some(origin) = self.imports.remove(&id) {
            if let Some(snapshot) = self.snapshots.get(&origin) {
                self.active_imports.insert(
                    id,
                    ImportedBindings {
                        frames: 1,
                        definitions: Rc::clone(snapshot),
                    },
                );
            }
            let remaining = self.remaining_imports.get_mut(&origin).unwrap();
            *remaining -= 1;
            if *remaining == 0 {
                self.snapshots.remove(&origin);
            }
        }
    }

    pub(super) fn exit(&mut self, scope: &mut LocalScope<'a>, depth: usize) {
        debug_assert_eq!(depth, self.scope_nodes.len());
        let id = self.scope_nodes.pop().unwrap();
        if self
            .remaining_imports
            .get(&id)
            .is_some_and(|count| *count > 0)
        {
            self.snapshots
                .entry(id)
                .or_insert_with(|| Rc::new(std::mem::take(&mut scope.local_defs)));
        }
        if let Some(imported) = self.active_imports.get_mut(&id) {
            imported.frames -= 1;
            if imported.frames == 0 {
                self.active_imports.remove(&id);
            }
        }
    }

    pub(super) fn bindings(&self, depth: usize) -> Option<&[LocalDef<'a>]> {
        depth
            .checked_sub(1)
            .and_then(|index| self.scope_nodes.get(index))
            .and_then(|id| self.active_imports.get(id))
            .map(|imported| imported.definitions.as_slice())
    }
}
