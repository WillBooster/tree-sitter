use super::{
    Error, Highlight, LocalDef, LocalDefinitions, LocalScope, ancestry::Ancestry,
    highlight_for_name, metadata,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ops::Range,
    str,
    sync::atomic::AtomicUsize,
};
use tree_sitter::{CaptureQuantifier, Language, Node, Query, QueryCursor, QueryError, Tree};

pub struct PredeclaredQuery {
    query: Query,
    region_capture: Option<u32>,
    definition_capture: Option<u32>,
    value_capture: Option<u32>,
    roles: Vec<Option<Box<str>>>,
    highlights: Vec<Option<Highlight>>,
}

struct PendingDefinition<'a> {
    pattern: usize,
    start: usize,
    explicit_end: Option<usize>,
    definition: LocalDef<'a>,
}

struct Region<'a> {
    id: usize,
    range: Range<usize>,
    definitions: Vec<LocalDef<'a>>,
    anchor: usize,
    owner_scope: bool,
}

struct PendingRegion<'a> {
    region: Region<'a>,
    definitions: HashMap<usize, PendingDefinition<'a>>,
}

pub struct PredeclaredLocals<'a> {
    pending: VecDeque<Region<'a>>,
    active: Vec<Region<'a>>,
    scope_nodes: Vec<usize>,
    root: usize,
    declarations: HashSet<usize>,
    ancestry: Ancestry,
}

impl PredeclaredQuery {
    pub(super) fn new(
        language: &Language,
        locals: &str,
        main_query: &Query,
        locals_offset: usize,
        source: &str,
    ) -> Result<Option<Self>, QueryError> {
        if main_query
            .capture_index_for_name("local.definition-region")
            .is_none()
            && main_query
                .capture_index_for_name("local.definition-predeclared")
                .is_none()
        {
            return Ok(None);
        }
        Self::compile(language, locals)
            .map_err(|error| metadata::remap_error(error, locals_offset, source))
    }

    pub(super) fn configure(&mut self, names: &[impl AsRef<str>]) {
        self.highlights = self
            .roles
            .iter()
            .map(|role| {
                role.as_deref()
                    .and_then(|role| highlight_for_name(role, names))
            })
            .collect();
    }

    pub(super) fn disable_capture(&mut self, name: &str) {
        self.query.disable_capture(name);
        match name {
            "local.definition-region" => self.region_capture = None,
            "local.definition-predeclared" => self.definition_capture = None,
            "local.definition-value" => self.value_capture = None,
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
        source: &'a [u8],
        values: &LocalDefinitions,
        cancellation: Option<&AtomicUsize>,
    ) -> Result<Option<Box<PredeclaredLocals<'a>>>, Error> {
        let (Some(region_capture), Some(definition_capture)) =
            (self.region_capture, self.definition_capture)
        else {
            return Ok(None);
        };
        let mut regions: HashMap<usize, PendingRegion<'a>> = HashMap::new();
        let mut declarations: HashMap<usize, (usize, Option<Highlight>)> = HashMap::new();
        metadata::for_each_match(&self.query, cursor, tree, source, cancellation, |mat| {
            let Some(owner) = mat
                .captures()
                .iter()
                .find(|capture| capture.index == region_capture)
                .map(|capture| capture.node)
            else {
                return;
            };
            let region = regions.entry(owner.id()).or_insert_with(|| PendingRegion {
                region: Region {
                    id: owner.id(),
                    range: owner.byte_range(),
                    definitions: Vec::new(),
                    anchor: 0,
                    owner_scope: false,
                },
                definitions: HashMap::new(),
            });
            let explicit_end = mat
                .captures()
                .iter()
                .filter(|capture| Some(capture.index) == self.value_capture)
                .map(|capture| capture.node.end_byte())
                .max();
            for capture in mat
                .captures()
                .iter()
                .filter(|capture| capture.index == definition_capture)
            {
                if metadata::is_cancelled(cancellation) {
                    return;
                }
                let node = capture.node;
                let Ok(name) = str::from_utf8(&source[node.byte_range()]) else {
                    continue;
                };
                if name.is_empty() {
                    continue;
                }
                let highlight = self.highlights[mat.pattern_index];
                let declaration = declarations
                    .entry(node.id())
                    .or_insert((mat.pattern_index, highlight));
                if mat.pattern_index >= declaration.0 {
                    *declaration = (mat.pattern_index, highlight);
                }
                let definition =
                    region
                        .definitions
                        .entry(node.id())
                        .or_insert_with(|| PendingDefinition {
                            pattern: mat.pattern_index,
                            start: node.start_byte(),
                            explicit_end,
                            definition: LocalDef {
                                name,
                                value_range: 0..0,
                                highlight,
                            },
                        });
                if mat.pattern_index > definition.pattern {
                    definition.pattern = mat.pattern_index;
                    definition.explicit_end = explicit_end;
                } else if mat.pattern_index == definition.pattern {
                    definition.explicit_end = definition.explicit_end.max(explicit_end);
                }
            }
        })?;
        if declarations.is_empty() {
            return Ok(None);
        }
        let targets: HashMap<_, _> = regions
            .iter()
            .flat_map(|(id, pending)| {
                std::iter::once((*id, pending.region.range.clone())).chain(
                    pending.definitions.iter().map(|(id, definition)| {
                        (
                            *id,
                            definition.start..definition.start + definition.definition.name.len(),
                        )
                    }),
                )
            })
            .collect();
        let ancestry = Ancestry::new(tree.root_node(), &targets, cancellation)?;
        PredeclaredLocals::new(
            regions,
            declarations,
            tree.root_node().id(),
            ancestry,
            values,
            cancellation,
        )
    }

    fn compile(language: &Language, locals: &str) -> Result<Option<Self>, QueryError> {
        let mut query = Query::new(language, locals)?;
        let region_capture = query.capture_index_for_name("local.definition-region");
        let definition_capture = query.capture_index_for_name("local.definition-predeclared");
        let value_capture = query.capture_index_for_name("local.definition-value");
        let mut roles = vec![None; query.pattern_count()];
        let mut enabled = false;
        for (pattern, stored_role) in roles.iter_mut().enumerate() {
            let quantifiers = query.capture_quantifiers(pattern);
            let region = region_capture.map_or(CaptureQuantifier::Zero, |capture| {
                quantifiers[capture as usize]
            });
            let definition = definition_capture.map_or(CaptureQuantifier::Zero, |capture| {
                quantifiers[capture as usize]
            });
            if region == CaptureQuantifier::Zero && definition == CaptureQuantifier::Zero {
                query.disable_pattern(pattern);
                continue;
            }
            if region != CaptureQuantifier::One
                || !matches!(
                    definition,
                    CaptureQuantifier::One | CaptureQuantifier::OneOrMore
                )
            {
                return Err(metadata::structure_error(
                    locals,
                    query.start_byte_for_pattern(pattern),
                    "Each predeclared definition pattern needs exactly one local.definition-region and one or more local.definition-predeclared nodes",
                ));
            }
            let role = query
                .property_settings(pattern)
                .iter()
                .rev()
                .find(|property| {
                    property.key.as_ref() == "local.definition-style"
                        && property
                            .capture_id
                            .is_none_or(|id| Some(id as u32) == definition_capture)
                })
                .and_then(|property| property.value.as_deref())
                .filter(|role| !role.is_empty());
            let Some(role) = role else {
                return Err(metadata::structure_error(
                    locals,
                    query.start_byte_for_pattern(pattern),
                    "Each predeclared definition pattern needs a nonempty local.definition-style",
                ));
            };
            *stored_role = Some(role.into());
            enabled = true;
        }
        if !enabled {
            return Ok(None);
        }
        let highlights = vec![None; query.pattern_count()];
        Ok(Some(Self {
            query,
            region_capture,
            definition_capture,
            value_capture,
            roles,
            highlights,
        }))
    }
}

impl<'a> PredeclaredLocals<'a> {
    fn new(
        regions: HashMap<usize, PendingRegion<'a>>,
        declarations: HashMap<usize, (usize, Option<Highlight>)>,
        root: usize,
        ancestry: Ancestry,
        values: &LocalDefinitions,
        cancellation: Option<&AtomicUsize>,
    ) -> Result<Option<Box<Self>>, Error> {
        let mut regions = regions.into_values().collect::<Vec<_>>();
        let mut definitions = regions
            .iter_mut()
            .flat_map(|pending| {
                let owner = pending.region.id;
                pending
                    .definitions
                    .iter_mut()
                    .map(move |(id, definition)| (owner, *id, definition))
            })
            .collect::<Vec<_>>();
        definitions.sort_unstable_by_key(|(_, _, definition)| definition.start);
        let mut values = values.context_cursor();
        for (owner, id, definition) in definitions {
            if metadata::is_cancelled(cancellation) {
                return Err(Error::Cancelled);
            }
            let range = definition.start..definition.start + definition.definition.name.len();
            let end = definition
                .explicit_end
                .or_else(|| {
                    values
                        .value_range(&range, ancestry.common_range(id, owner))
                        .map(|range| range.end)
                })
                .unwrap_or(0);
            definition.definition.value_range = 0..end;
            definition.definition.highlight = declarations[&id].1;
        }
        let mut regions = regions
            .into_iter()
            .filter_map(|mut pending| {
                let mut definitions = pending.definitions.into_iter().collect::<Vec<_>>();
                definitions.sort_unstable_by_key(|(id, definition)| {
                    (definition.start, definition.pattern, *id)
                });
                pending.region.definitions = definitions
                    .into_iter()
                    .map(|(_, definition)| definition.definition)
                    .collect();
                (!pending.region.definitions.is_empty()).then_some(pending.region)
            })
            .collect::<Vec<_>>();
        if regions.is_empty() {
            return Ok(None);
        }
        regions.sort_unstable_by_key(|region| {
            (
                region.range.start,
                std::cmp::Reverse(region.range.end),
                ancestry.depth(region.id),
            )
        });
        Ok(Some(Box::new(PredeclaredLocals {
            pending: regions.into(),
            active: Vec::new(),
            scope_nodes: Vec::new(),
            root,
            declarations: declarations.into_keys().collect(),
            ancestry,
        })))
    }

    pub(super) fn advance(&mut self, start: usize) {
        self.active.retain(|region| start < region.range.end);
        while self
            .pending
            .front()
            .is_some_and(|region| region.range.start <= start)
        {
            let mut region = self.pending.pop_front().unwrap();
            if region.range.end <= start {
                continue;
            }
            region.anchor = self
                .scope_nodes
                .iter()
                .rposition(|id| self.ancestry.contains(*id, region.id))
                .map_or(0, |index| index + 1);
            region.owner_scope = region.id == self.root && region.anchor == 0;
            if let Some(index) = self.scope_nodes.iter().position(|id| *id == region.id) {
                region.anchor = index + 1;
                region.owner_scope = true;
            }
            self.active.push(region);
        }
    }

    pub(super) fn enter(&mut self, node: Node, depth: usize) {
        debug_assert_eq!(depth, self.scope_nodes.len() + 1);
        self.scope_nodes.push(node.id());
        for region in &mut self.active {
            if region.id == node.id() {
                if !region.owner_scope {
                    region.anchor = depth;
                    region.owner_scope = true;
                }
            } else if !region.owner_scope && self.ancestry.contains(node.id(), region.id) {
                region.anchor = depth;
            }
        }
    }

    pub(super) fn exit(&mut self, depth: usize) {
        debug_assert_eq!(depth, self.scope_nodes.len());
        self.scope_nodes.pop();
    }

    pub(super) fn is_declaration(&self, node: Node) -> bool {
        self.declarations.contains(&node.id())
    }

    pub(super) fn before(&self, depth: usize, name: &str, start: usize) -> Option<&LocalDef<'a>> {
        self.active
            .iter()
            .rev()
            .filter(|region| region.anchor == depth && !region.owner_scope)
            .find_map(|region| super::find_local_definition(&region.definitions, name, start))
    }

    pub(super) fn after<'lookup>(
        &'lookup self,
        scopes: &'lookup [LocalScope<'a>],
        depth: usize,
        name: &str,
        start: usize,
    ) -> Option<&'lookup LocalDef<'a>> {
        let id = depth
            .checked_sub(1)
            .and_then(|index| self.scope_nodes.get(index))
            .copied()
            .unwrap_or(self.root);
        let declaration = self
            .active
            .iter()
            .rev()
            .filter(|region| region.owner_scope && region.id == id)
            .find_map(|region| super::find_local_definition(&region.definitions, name, start))?;
        let mut frame = depth;
        while frame > 0 && self.scope_nodes[frame - 1] == id {
            if let Some(definition) =
                super::find_local_definition(&scopes[frame].local_defs, name, start)
            {
                return Some(definition);
            }
            frame -= 1;
        }
        Some(declaration)
    }
}
