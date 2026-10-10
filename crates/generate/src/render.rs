use std::{
    cmp,
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    mem::swap,
};

use indexmap::IndexMap;
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::tables::{ActionListId, ActionListPool};

use super::{
    LANGUAGE_VERSION,
    build_tables::Tables,
    grammars::{LexicalGrammar, SyntaxGrammar, VariableType},
    nfa::CharacterSet,
    node_types::ChildType,
    rules::Alias,
    rules::{AliasMap, Symbol, SymbolType, TokenSet},
    strpool::{StrId, StrPool},
    tables::{AdvanceAction, GotoAction, LexState, LexTable, ParseAction, ParseTable},
};

const SMALL_STATE_THRESHOLD: usize = 64;
const BITMAP_STATE_MIN_ENTRIES: usize = 16;
const SMALL_STATE_BITMAP_FLAG: usize = 0x4000_0000;
const SMALL_STATE_PAIR_FLAG: usize = 0x8000_0000;
const MAX_SINGLE_LEXER_STATES: usize = 4096;
const LEXER_CHUNK_SIZE: usize = 256;
const KEYWORD_SKIP_FLAG: usize = 1 << 15;
pub const ABI_VERSION_MIN: usize = 14;
pub const ABI_VERSION_MAX: usize = LANGUAGE_VERSION;
pub const ABI_VERSION_DEFAULT: usize = 15;
const ABI_VERSION_WITH_RESERVED_WORDS: usize = 15;
pub const ABI_VERSION_WITH_COMPACT_TABLES: usize = 16;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationProfile {
    pub fingerprint: String,
    pub parse_states: Vec<u64>,
    pub lex_states: Vec<u64>,
    pub edges: Vec<(u32, u32, u64)>,
    pub max_dense_states: usize,
}

impl GenerationProfile {
    pub const SOURCE_MARKER: &'static str = "#define TS_GENERATION_PROFILED 1";
}

#[must_use]
pub fn parser_fingerprint(source: &str) -> String {
    let hash = source
        .bytes()
        .fold(14_695_981_039_346_656_037u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211)
        });
    format!("{hash:016x}")
}

pub type RenderResult<T> = Result<T, RenderError>;

#[derive(Debug, Error, Serialize, Deserialize)]
pub enum RenderError {
    #[error("Sparse parse table offset {0} exceeds the ABI 16 address limit")]
    SparseTable(usize),
    #[error("Invalid generation profile: {0}")]
    Profile(String),
    #[error("Parse table action count {0} exceeds maximum value of {max}", max=u16::MAX)]
    ParseTable(usize),
    #[error(
        "This version of Tree-sitter can only generate parsers with ABI version {ABI_VERSION_MIN} - {ABI_VERSION_MAX}, not {0}"
    )]
    ABI(usize),
}

#[clippy::format_args]
macro_rules! add {
    ($this: tt, $($arg: tt)*) => {{
        $this.buffer.write_fmt(format_args!($($arg)*)).unwrap();
    }}
}

macro_rules! add_whitespace {
    ($this:tt) => {{
        for _ in 0..$this.indent_level {
            write!(&mut $this.buffer, "  ").unwrap();
        }
    }};
}

#[clippy::format_args]
macro_rules! add_line {
    ($this: tt, $($arg: tt)*) => {
        add_whitespace!($this);
        $this.buffer.write_fmt(format_args!($($arg)*)).unwrap();
        $this.buffer += "\n";
    }
}

macro_rules! indent {
    ($this:tt) => {
        $this.indent_level += 1;
    };
}

macro_rules! dedent {
    ($this:tt) => {
        assert_ne!($this.indent_level, 0);
        $this.indent_level -= 1;
    };
}

#[derive(Default)]
struct Generator {
    buffer: String,
    indent_level: usize,
    language_name: String,
    parse_table: ParseTable,
    main_lex_table: LexTable,
    keyword_lex_table: LexTable,
    large_character_sets: Vec<(Option<Symbol>, CharacterSet)>,
    large_character_set_info: Vec<LargeCharacterSetInfo>,
    large_state_count: usize,
    alias_sequence_count: usize,
    lex_mode_count: usize,
    uses_reserved_word_slices: bool,
    external_state_stride: usize,
    uses_keyword_table: bool,
    profile: Option<GenerationProfile>,
    advance_maps: IndexMap<Vec<(char, u32)>, usize, FxBuildHasher>,
    ascii_sets: IndexMap<[u32; 4], usize, FxBuildHasher>,
    uses_lex_repeat: bool,
    syntax_grammar: SyntaxGrammar,
    lexical_grammar: LexicalGrammar,
    default_aliases: AliasMap,
    symbol_order: FxHashMap<Symbol, usize>,
    symbol_ids: FxHashMap<Symbol, String>,
    alias_ids: FxHashMap<Alias, String>,
    unique_aliases: Vec<Alias>,
    symbol_map: FxHashMap<Symbol, Symbol>,
    symbols_by_node_identity: FxHashMap<(StrId, VariableType), Vec<Symbol>>,
    reserved_word_sets: Vec<TokenSet>,
    reserved_word_set_ids_by_parse_state: Vec<usize>,
    field_names: Vec<StrId>,
    supertype_symbol_map: BTreeMap<Symbol, Vec<ChildType>>,
    supertype_map: BTreeMap<String, Vec<ChildType>>,
    abi_version: usize,
    metadata: Option<Metadata>,
    str_pool: StrPool,
}

struct LargeCharacterSetInfo {
    constant_name: String,
    is_used: bool,
    unicode_pages: Option<UnicodePages>,
}

struct UnicodePages {
    first_page: u32,
    page_ids: Vec<u16>,
    blocks: Vec<[u32; 8]>,
}

#[derive(Clone, Copy, Default)]
struct Metadata {
    major: u8,
    minor: u8,
    patch: u8,
}

impl Generator {
    fn generate(mut self) -> RenderResult<String> {
        self.apply_profile()?;
        self.init();
        self.add_header();
        self.add_includes();
        self.add_pragmas();
        self.add_stats();
        self.add_symbol_enum();
        self.add_symbol_names_list();
        self.add_unique_symbol_map();
        self.add_symbol_metadata_list();

        if !self.field_names.is_empty() {
            self.add_field_name_enum();
            self.add_field_name_names_list();
            self.add_field_sequences();
        }

        if !self.parse_table.production_infos.is_empty() {
            self.add_alias_sequences();
        }

        self.add_non_terminal_alias_map();
        self.add_primary_state_id_list();

        if self.abi_version >= ABI_VERSION_WITH_RESERVED_WORDS && !self.supertype_map.is_empty() {
            self.add_supertype_map();
        }

        let buffer_offset_before_lex_functions = self.buffer.len();

        let mut main_lex_table = LexTable::default();
        swap(&mut main_lex_table, &mut self.main_lex_table);
        self.add_lex_function("ts_lex", main_lex_table);

        if self.syntax_grammar.word_token.is_some() {
            let mut keyword_lex_table = LexTable::default();
            swap(&mut keyword_lex_table, &mut self.keyword_lex_table);
            if self.abi_version >= ABI_VERSION_WITH_COMPACT_TABLES
                && keyword_lex_table.states.len() >= 64
                && keyword_lex_table.states.len() < KEYWORD_SKIP_FLAG
                && keyword_lex_table.states.iter().all(|state| {
                    state.eof_action.is_none()
                        && state.advance_actions.iter().all(|(set, _)| {
                            set.ranges()
                                .all(|range| *range.start() > '\0' && range.end().is_ascii())
                        })
                })
            {
                self.uses_keyword_table = true;
                self.add_keyword_table(&keyword_lex_table);
            } else {
                self.add_lex_function("ts_lex_keywords", keyword_lex_table);
            }
        }

        let lex_functions = self.buffer[buffer_offset_before_lex_functions..].to_string();
        self.buffer.truncate(buffer_offset_before_lex_functions);
        self.add_lexer_helpers();
        for ix in 0..self.large_character_sets.len() {
            self.add_character_set(ix);
        }
        self.buffer.push_str(&lex_functions);

        self.add_lex_modes();

        if self.abi_version >= ABI_VERSION_WITH_RESERVED_WORDS && self.reserved_word_sets.len() > 1
        {
            self.add_reserved_word_sets();
        }

        self.add_parse_table()?;

        if !self.syntax_grammar.external_tokens.is_empty() {
            self.add_external_token_enum();
            self.add_external_scanner_symbol_map();
            self.add_external_scanner_states_list();
        }

        self.add_parser_export();

        Ok(self.buffer)
    }

    fn apply_profile(&mut self) -> RenderResult<()> {
        let Some(profile) = &mut self.profile else {
            return Ok(());
        };
        let n = self.parse_table.states.len();
        if self.abi_version < ABI_VERSION_WITH_COMPACT_TABLES
            || profile.parse_states.len() != n
            || profile.lex_states.len() != self.main_lex_table.states.len()
            || profile
                .edges
                .iter()
                .any(|&(a, b, _)| a as usize >= n || b as usize >= n)
            || !(2..=n).contains(&profile.max_dense_states)
        {
            return Err(RenderError::Profile("requires ABI 16, matching state counts, valid edges, and a dense-state limit between 2 and STATE_COUNT".into()));
        }
        let mut order = (2..n).collect::<Vec<_>>();
        order.sort_unstable_by_key(|&i| (std::cmp::Reverse(profile.parse_states[i]), i));
        let mut available = order
            .iter()
            .enumerate()
            .map(|(rank, &id)| (id, rank))
            .collect::<FxHashMap<_, _>>();
        let mut edges = vec![Vec::new(); n];
        for &(a, b, count) in &profile.edges {
            if b > 1 {
                edges[a as usize].push((b as usize, count));
            }
        }
        for row in &mut edges {
            row.sort_unstable_by_key(|&(id, count)| (std::cmp::Reverse(count), id));
        }
        let mut ranked = vec![0, 1];
        let mut cursor = 0;
        while !available.is_empty() {
            while cursor < order.len() && !available.contains_key(&order[cursor]) {
                cursor += 1;
            }
            let hottest = order[cursor];
            let previous = *ranked.last().unwrap();
            let next = edges[previous]
                .iter()
                .find_map(|&(id, count)| {
                    (available.contains_key(&id)
                        && (profile.parse_states[id] > 0 || profile.parse_states[hottest] == 0)
                        && u128::from(count) * 2 >= u128::from(profile.parse_states[hottest]))
                    .then_some(id)
                })
                .unwrap_or(hottest);
            available.remove(&next);
            ranked.push(next);
        }
        let mut inverse = vec![0u32; n];
        for (new, &old) in ranked.iter().enumerate() {
            inverse[old] = new as u32;
        }
        self.parse_table.states = ranked
            .iter()
            .map(|&old| {
                let mut state = std::mem::take(&mut self.parse_table.states[old]);
                state.update_nonterminal_references(|id, _| inverse[id as usize]);
                state
            })
            .collect();
        self.parse_table
            .remap_terminal_references(|id| inverse[id as usize]);
        profile.parse_states = ranked
            .iter()
            .map(|&old| profile.parse_states[old])
            .collect();
        let mut lex_order = (0..self.main_lex_table.states.len()).collect::<Vec<_>>();
        lex_order[1..].sort_unstable_by_key(|&i| (std::cmp::Reverse(profile.lex_states[i]), i));
        let mut lex_inverse = vec![0; lex_order.len()];
        for (new, &old) in lex_order.iter().enumerate() {
            lex_inverse[old] = new as u32;
        }
        self.main_lex_table.states = lex_order
            .iter()
            .map(|&old| {
                let mut state = std::mem::take(&mut self.main_lex_table.states[old]);
                if let Some(action) = &mut state.eof_action {
                    action.state = lex_inverse[action.state as usize];
                }
                for (_, action) in &mut state.advance_actions {
                    action.state = lex_inverse[action.state as usize];
                }
                state
            })
            .collect();
        for state in &mut self.parse_table.states {
            if state.lex_state_id != u32::MAX {
                state.lex_state_id = lex_inverse[state.lex_state_id as usize];
            }
        }
        Ok(())
    }

    fn add_keyword_table(&mut self, table: &LexTable) {
        add_line!(self, "#define TS_KEYWORD_SKIP {KEYWORD_SKIP_FLAG}u");
        add_line!(
            self,
            "#define TS_KEYWORD_STATE_MASK {}u",
            KEYWORD_SKIP_FLAG - 1
        );
        add_line!(
            self,
            "typedef struct {{ uint8_t first, last; uint16_t state; }} TSKeywordTransition;"
        );
        add_line!(
            self,
            "static const TSKeywordTransition ts_keyword_transitions[] = {{"
        );
        indent!(self);
        let mut transitions = Vec::new();
        let mut rows = Vec::new();
        for state in &table.states {
            let index = transitions.len();
            let mut ranges = state
                .advance_actions
                .iter()
                .flat_map(|(set, action)| {
                    let destination = action.state
                        | if action.in_main_token {
                            0
                        } else {
                            KEYWORD_SKIP_FLAG as u32
                        };
                    set.ranges()
                        .map(move |range| (*range.start() as u8, *range.end() as u8, destination))
                })
                .collect::<Vec<_>>();
            ranges.sort_unstable();
            rows.push((index, state.accept_action));
            transitions.extend(ranges);
        }
        let transition_count = transitions.len();
        for (first, last, state) in transitions {
            add_line!(self, "{{ {first}, {last}, {state} }},");
        }
        dedent!(self);
        add_line!(self, "}};");
        let index_width = if u16::try_from(transition_count).is_ok() {
            16
        } else {
            32
        };
        add_line!(
            self,
            "typedef struct {{ uint{index_width}_t index; TSSymbol accept; }} TSKeywordState;"
        );
        add_line!(self, "static const TSKeywordState ts_keyword_states[] = {{");
        indent!(self);
        for (index, accept) in rows {
            let accept = accept.map_or_else(
                || "0".to_string(),
                |symbol| self.symbol_ids[&symbol].clone(),
            );
            add_line!(self, "{{ {index}, {accept} }},");
        }
        add_line!(self, "{{ {transition_count}, 0 }},");
        dedent!(self);
        add_line!(self, "}};");
        self.buffer
            .push_str(include_str!("templates/keyword_table.h"));
    }

    fn add_lexer_helpers(&mut self) {
        if self
            .large_character_set_info
            .iter()
            .any(|info| info.is_used && info.unicode_pages.is_none())
        {
            self.buffer
                .push_str(include_str!("templates/character_set.h"));
        }
        for width in [8, 16] {
            if self.large_character_set_info.iter().any(|info| {
                info.is_used
                    && info.unicode_pages.as_ref().is_some_and(|pages| {
                        width == if pages.blocks.len() <= 256 { 8 } else { 16 }
                    })
            }) {
                self.buffer.push_str(
                    &include_str!("templates/character_set_pages.h")
                        .replace("WIDTH", &width.to_string()),
                );
            }
        }
        let advance_maps = std::mem::take(&mut self.advance_maps);
        for (map, &id) in &advance_maps {
            let (first, span, dense) = Self::advance_map_layout(map);
            add_line!(self, "static const uint16_t ts_lex_advance_map_{id}[] = {{");
            indent!(self);
            if dense {
                let mut transitions = map.iter().peekable();
                for character in first..first + span {
                    if let Some(&(next_character, state)) = transitions.peek()
                        && *next_character as u32 == character
                    {
                        add_line!(self, "{state},");
                        transitions.next();
                    } else {
                        add_line!(self, "UINT16_MAX,");
                    }
                }
            } else {
                for &(character, state) in map {
                    add_whitespace!(self);
                    self.add_character(character);
                    add!(self, ", {state},\n");
                }
            }
            dedent!(self);
            add_line!(self, "}};\n");
        }
        if !advance_maps.is_empty() {
            self.buffer
                .push_str(include_str!("templates/advance_map.h"));
        }
        let ascii_sets = std::mem::take(&mut self.ascii_sets);
        for (set, &id) in &ascii_sets {
            add_line!(
                self,
                "static const uint32_t ts_lex_ascii_set_{id}[] = {{0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}}};",
                set[0],
                set[1],
                set[2],
                set[3]
            );
        }
        if !ascii_sets.is_empty() {
            self.buffer.push_str(include_str!("templates/ascii_set.h"));
        }
        if self.uses_lex_repeat {
            self.buffer.push_str(include_str!("templates/lex_repeat.h"));
        }
    }

    fn init(&mut self) {
        let mut symbol_identifiers = FxHashSet::default();
        let mut symbol_identifier_suffixes = FxHashMap::default();
        for i in 0..self.parse_table.symbols.len() {
            self.assign_symbol_id(
                self.parse_table.symbols[i],
                &mut symbol_identifiers,
                &mut symbol_identifier_suffixes,
            );
        }
        self.symbol_ids.insert(
            Symbol::end_of_nonterminal_extra(),
            self.symbol_ids[&Symbol::end()].clone(),
        );

        let mut first_symbol_by_metadata = FxHashMap::default();
        let mut first_unaliased_symbol_by_metadata = FxHashMap::default();
        let mut minimum_symbol_by_alias = FxHashMap::default();
        for &symbol in &self.parse_table.symbols {
            let metadata = self.metadata_for_symbol(symbol);
            first_symbol_by_metadata.entry(metadata).or_insert(symbol);
            let identity = if let Some(&alias) = self.default_aliases.get(&symbol) {
                let minimum = minimum_symbol_by_alias.entry(alias).or_insert(symbol);
                *minimum = (*minimum).min(symbol);
                (alias.value, alias.kind())
            } else {
                first_unaliased_symbol_by_metadata
                    .entry(metadata)
                    .or_insert(symbol);
                metadata
            };
            self.symbols_by_node_identity
                .entry(identity)
                .or_default()
                .push(symbol);
        }

        for symbol in &self.parse_table.symbols {
            let mapping = if let Some(target) = self
                .syntax_grammar
                .supertype_alias(*symbol, &self.default_aliases)
            {
                target
            } else if let Some(alias) = self.default_aliases.get(symbol) {
                first_unaliased_symbol_by_metadata
                    .get(&(alias.value, alias.kind()))
                    .copied()
                    .unwrap_or_else(|| minimum_symbol_by_alias[alias])
            } else if symbol.is_terminal() {
                let first = first_symbol_by_metadata[&self.metadata_for_symbol(*symbol)];
                // Avoid making a cycle when the first symbol already maps to this one.
                if self.symbol_map.get(&first) == Some(symbol) {
                    *symbol
                } else {
                    first
                }
            } else {
                *symbol
            };
            self.symbol_map.insert(*symbol, mapping);
        }

        let mut field_names = FxHashSet::default();
        for production_info in &self.parse_table.production_infos {
            field_names.extend(production_info.field_map.keys().copied());
            for &alias in production_info.alias_sequence.iter().flatten() {
                if self.alias_ids.contains_key(&alias) {
                    continue;
                }
                let alias_id = if let Some(existing_symbol) = self.symbols_for_alias(alias).first()
                {
                    self.symbol_ids[&self.symbol_map[existing_symbol]].clone()
                } else {
                    self.unique_aliases.push(alias);

                    if alias.is_named {
                        format!("alias_sym_{}", self.sanitize_identifier(alias.value))
                    } else {
                        format!("anon_alias_sym_{}", self.sanitize_identifier(alias.value))
                    }
                };

                self.alias_ids.insert(alias, alias_id);
            }
        }
        self.field_names.extend(field_names);
        self.field_names
            .sort_unstable_by(|&a, &b| self.str_pool.resolve(a).cmp(self.str_pool.resolve(b)));
        self.unique_aliases.sort_unstable_by(|a, b| {
            self.str_pool
                .resolve(a.value)
                .cmp(self.str_pool.resolve(b.value))
                .then_with(|| a.is_named.cmp(&b.is_named))
        });

        let mut character_set_counts = FxHashMap::default();
        for (symbol, characters) in &self.large_character_sets {
            let count = character_set_counts.entry(*symbol).or_insert(0);
            *count += 1;
            let constant_name = if let Some(symbol) = symbol {
                format!("ts_lex_{}_character_set_{}", self.symbol_ids[symbol], count)
            } else {
                format!("ts_lex_extras_character_set_{count}")
            };
            self.large_character_set_info.push(LargeCharacterSetInfo {
                constant_name,
                is_used: false,
                unicode_pages: Self::unicode_pages(characters),
            });
        }

        let empty_reserved_words = TokenSet::new();
        let mut reserved_word_set_ids = FxHashMap::default();
        reserved_word_set_ids.insert(&empty_reserved_words, 0);
        self.reserved_word_sets.push(empty_reserved_words.clone());
        for state in &self.parse_table.states {
            let id = *reserved_word_set_ids
                .entry(&state.reserved_words)
                .or_insert_with(|| {
                    self.reserved_word_sets.push(state.reserved_words.clone());
                    self.reserved_word_sets.len() - 1
                });
            self.reserved_word_set_ids_by_parse_state.push(id);
        }

        if self.abi_version >= ABI_VERSION_WITH_RESERVED_WORDS {
            for (supertype, subtypes) in &self.supertype_symbol_map {
                if let Some(supertype) = self.symbol_ids.get(supertype) {
                    self.supertype_map
                        .entry(supertype.clone())
                        .or_insert_with(|| subtypes.clone());
                }
            }

            self.supertype_symbol_map.clear();
        }

        self.large_state_count = self.count_large_states();
    }

    fn count_large_states(&self) -> usize {
        if let Some(profile) = &self.profile {
            let total = profile
                .parse_states
                .iter()
                .map(|&n| u128::from(n))
                .sum::<u128>();
            let mut count = 2;
            let mut covered =
                u128::from(profile.parse_states[0]) + u128::from(profile.parse_states[1]);
            while count < profile.max_dense_states && covered * 10 < total * 9 {
                covered += u128::from(profile.parse_states[count]);
                count += 1;
            }
            return count;
        }
        let threshold = cmp::min(SMALL_STATE_THRESHOLD, self.parse_table.symbols.len() / 2);
        let minimum = if self.abi_version >= ABI_VERSION_WITH_COMPACT_TABLES {
            self.parse_table.states.len().min(2)
        } else {
            self.parse_table
                .states
                .iter()
                .enumerate()
                .take_while(|(i, s)| {
                    *i <= 1 || s.terminal_entries.len() + s.nonterminal_entries.len() > threshold
                })
                .count()
        };
        let dense_bytes = self.parse_table.symbols.len() * size_of::<u16>();
        let mut terminal_groups = FxHashSet::default();
        let mut nonterminal_groups = FxHashSet::default();
        let mut row_ids = FxHashMap::default();
        let mut rows = Vec::new();
        let mut row_counts = Vec::new();
        let mut row_bytes = Vec::new();
        for (i, state) in self.parse_table.states.iter().enumerate().skip(minimum) {
            terminal_groups.clear();
            terminal_groups.extend(state.terminal_entries.values().copied());
            nonterminal_groups.clear();
            nonterminal_groups.extend(
                state
                    .nonterminal_entries
                    .values()
                    .map(|action| self.small_goto_value(*action, i)),
            );
            let entries = state.terminal_entries.len() + state.nonterminal_entries.len();
            let groups = terminal_groups.len() + nonterminal_groups.len();
            let mut row = state
                .terminal_entries
                .iter()
                .map(|(&symbol, id)| (symbol, (id.index() as u32) * 2 + u32::from(id.reusable())))
                .chain(
                    state
                        .nonterminal_entries
                        .iter()
                        .map(|(&symbol, action)| (symbol, self.small_goto_value(*action, i))),
                )
                .collect::<Vec<_>>();
            row.sort_unstable_by_key(|&(symbol, _)| symbol);
            let next_id = row_counts.len();
            let id = *row_ids.entry(row).or_insert_with(|| {
                row_counts.push(0usize);
                let (_, words) = self.small_state_encoding(entries, groups, false);
                row_bytes.push(words * size_of::<u16>());
                next_id
            });
            row_counts[id] += 1;
            rows.push(id);
        }
        let mut bytes =
            minimum * dense_bytes + rows.len() * size_of::<u32>() + row_bytes.iter().sum::<usize>();
        let mut best_bytes = bytes;
        let mut count = minimum;
        for (i, &id) in rows.iter().enumerate() {
            bytes += dense_bytes;
            bytes -= size_of::<u32>();
            row_counts[id] -= 1;
            if row_counts[id] == 0 {
                bytes -= row_bytes[id];
            }
            if bytes <= best_bytes {
                best_bytes = bytes;
                count = minimum + i + 1;
            }
        }
        count
    }

    fn add_header(&mut self) {
        add_line!(self, "/* Automatically @generated by tree-sitter */");
        add_line!(self, "");
    }

    fn add_includes(&mut self) {
        add_line!(self, "#include \"tree_sitter/parser.h\"");
        add_line!(self, "");
    }

    fn add_pragmas(&mut self) {
        add_line!(self, "#if defined(__GNUC__) || defined(__clang__)");
        add_line!(
            self,
            "#pragma GCC diagnostic ignored \"-Wmissing-field-initializers\""
        );
        add_line!(self, "#endif");
        add_line!(self, "");
    }

    fn add_stats(&mut self) {
        if self.profile.is_some() {
            add_line!(self, "{}", GenerationProfile::SOURCE_MARKER);
        }
        let token_count = self
            .parse_table
            .symbols
            .iter()
            .filter(|symbol| {
                if symbol.is_terminal() || symbol.is_eof() {
                    true
                } else if symbol.is_external() {
                    self.syntax_grammar.external_tokens[symbol.index as usize]
                        .corresponding_internal_token
                        .is_none()
                } else {
                    false
                }
            })
            .count();

        add_line!(self, "#define LANGUAGE_VERSION {}", self.abi_version);
        add_line!(
            self,
            "#define STATE_COUNT {}",
            self.parse_table.states.len()
        );
        add_line!(self, "#define LARGE_STATE_COUNT {}", self.large_state_count);
        if self.abi_version >= ABI_VERSION_WITH_COMPACT_TABLES {
            add_line!(
                self,
                "#define LEX_STATE_COUNT {}",
                self.main_lex_table.states.len()
            );
        }

        add_line!(
            self,
            "#define SYMBOL_COUNT {}",
            self.parse_table.symbols.len()
        );
        add_line!(self, "#define ALIAS_COUNT {}", self.unique_aliases.len());
        add_line!(self, "#define TOKEN_COUNT {token_count}");
        add_line!(
            self,
            "#define EXTERNAL_TOKEN_COUNT {}",
            self.syntax_grammar.external_tokens.len()
        );
        add_line!(self, "#define FIELD_COUNT {}", self.field_names.len());
        add_line!(
            self,
            "#define MAX_ALIAS_SEQUENCE_LENGTH {}",
            self.parse_table.max_aliased_production_length
        );
        add_line!(
            self,
            "#define MAX_RESERVED_WORD_SET_SIZE {}",
            self.reserved_word_sets
                .iter()
                .map(TokenSet::len)
                .max()
                .unwrap()
        );

        add_line!(
            self,
            "#define PRODUCTION_ID_COUNT {}",
            self.parse_table.production_infos.len()
        );
        add_line!(self, "#define SUPERTYPE_COUNT {}", self.supertype_map.len());
        add_line!(self, "");
    }

    fn add_symbol_enum(&mut self) {
        add_line!(self, "enum ts_symbol_identifiers {{");
        indent!(self);
        self.symbol_order.insert(Symbol::end(), 0);
        self.symbol_order
            .insert(Symbol::end_of_nonterminal_extra(), 0);
        let mut i = 1;
        for symbol in &self.parse_table.symbols {
            if *symbol != Symbol::end() {
                self.symbol_order.insert(*symbol, i);
                add_line!(self, "{} = {i},", self.symbol_ids[symbol]);
                i += 1;
            }
        }
        for alias in &self.unique_aliases {
            add_line!(self, "{} = {i},", self.alias_ids[alias]);
            i += 1;
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_symbol_names_list(&mut self) {
        add_line!(self, "static const char * const ts_symbol_names[] = {{");
        indent!(self);
        for symbol in &self.parse_table.symbols {
            let name = self.sanitize_string(
                self.default_aliases
                    .get(symbol)
                    .map_or_else(|| self.metadata_for_symbol(*symbol).0, |alias| alias.value),
            );
            add_line!(self, "[{}] = \"{name}\",", self.symbol_ids[symbol]);
        }
        for alias in &self.unique_aliases {
            add_line!(
                self,
                "[{}] = \"{}\",",
                self.alias_ids[alias],
                self.sanitize_string(alias.value)
            );
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_unique_symbol_map(&mut self) {
        add_line!(self, "static const TSSymbol ts_symbol_map[] = {{");
        indent!(self);
        for symbol in &self.parse_table.symbols {
            add_line!(
                self,
                "[{}] = {},",
                self.symbol_ids[symbol],
                self.symbol_ids[&self.symbol_map[symbol]],
            );
        }

        for alias in &self.unique_aliases {
            add_line!(
                self,
                "[{}] = {},",
                self.alias_ids[alias],
                self.alias_ids[alias],
            );
        }

        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_field_name_enum(&mut self) {
        add_line!(self, "enum ts_field_identifiers {{");
        indent!(self);
        for (i, &field_name) in self.field_names.iter().enumerate() {
            add_line!(
                self,
                "{} = {},",
                Self::field_id(self.str_pool.resolve(field_name)),
                i + 1
            );
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_field_name_names_list(&mut self) {
        add_line!(self, "static const char * const ts_field_names[] = {{");
        indent!(self);
        add_line!(self, "[0] = NULL,");
        for &field_name in &self.field_names {
            let field_name = self.str_pool.resolve(field_name);
            add_line!(self, "[{}] = \"{field_name}\",", Self::field_id(field_name));
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_symbol_metadata_list(&mut self) {
        add_line!(
            self,
            "static const TSSymbolMetadata ts_symbol_metadata[] = {{"
        );
        indent!(self);
        for symbol in &self.parse_table.symbols {
            add_line!(self, "[{}] = {{", self.symbol_ids[symbol]);
            indent!(self);
            if self
                .syntax_grammar
                .supertype_alias(*symbol, &self.default_aliases)
                .is_some()
            {
                add_line!(self, ".visible = false,");
                add_line!(self, ".named = true,");
                add_line!(self, ".supertype = true,");
            } else if let Some(Alias { is_named, .. }) = self.default_aliases.get(symbol) {
                add_line!(self, ".visible = true,");
                add_line!(self, ".named = {is_named},");
            } else {
                match self.metadata_for_symbol(*symbol).1 {
                    VariableType::Named => {
                        add_line!(self, ".visible = true,");
                        add_line!(self, ".named = true,");
                    }
                    VariableType::Anonymous => {
                        add_line!(self, ".visible = true,");
                        add_line!(self, ".named = false,");
                    }
                    VariableType::Hidden => {
                        add_line!(self, ".visible = false,");
                        add_line!(self, ".named = true,");
                        if self.syntax_grammar.supertype_symbols.contains(symbol) {
                            add_line!(self, ".supertype = true,");
                        }
                    }
                    VariableType::Auxiliary => {
                        add_line!(self, ".visible = false,");
                        add_line!(self, ".named = false,");
                    }
                }
            }
            dedent!(self);
            add_line!(self, "}},");
        }
        for alias in &self.unique_aliases {
            add_line!(self, "[{}] = {{", self.alias_ids[alias]);
            indent!(self);
            add_line!(self, ".visible = true,");
            add_line!(self, ".named = {},", alias.is_named);
            dedent!(self);
            add_line!(self, "}},");
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_alias_sequences(&mut self) {
        if self.abi_version >= ABI_VERSION_WITH_COMPACT_TABLES {
            self.add_shared_alias_sequences();
            return;
        }

        add_line!(
            self,
            "static const TSSymbol ts_alias_sequences[PRODUCTION_ID_COUNT][MAX_ALIAS_SEQUENCE_LENGTH] = {{",
        );
        indent!(self);
        for (i, production_info) in self.parse_table.production_infos.iter().enumerate() {
            if production_info.alias_sequence.is_empty() {
                // Work around MSVC's intolerance of empty array initializers by
                // explicitly zero-initializing the first element.
                if i == 0 {
                    add_line!(self, "[0] = {{0}},");
                }
                continue;
            }

            add_line!(self, "[{i}] = {{");
            indent!(self);
            for (j, alias) in production_info.alias_sequence.iter().enumerate() {
                if let Some(alias) = alias {
                    add_line!(self, "[{j}] = {},", self.alias_ids[alias]);
                }
            }
            dedent!(self);
            add_line!(self, "}},");
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_shared_alias_sequences(&mut self) {
        let mut rows = IndexMap::<Vec<Option<Alias>>, usize, FxBuildHasher>::default();
        let mut symbols = vec![None];
        let mut offsets = Vec::new();
        for info in &self.parse_table.production_infos {
            if info.alias_sequence.is_empty() {
                offsets.push(0);
                continue;
            }
            let mut row = info.alias_sequence.clone();
            row.resize(self.parse_table.max_aliased_production_length, None);
            let offset = *rows.entry(row.clone()).or_insert_with(|| {
                let offset = symbols.len();
                symbols.extend(row);
                offset
            });
            offsets.push(offset);
        }
        self.alias_sequence_count = symbols.len();
        add_line!(
            self,
            "static const uint32_t ts_alias_sequence_offsets[PRODUCTION_ID_COUNT] = {{"
        );
        indent!(self);
        for (id, offset) in offsets.into_iter().enumerate() {
            add_line!(self, "[{id}] = {offset},");
        }
        dedent!(self);
        add_line!(self, "}};\n");
        add_line!(
            self,
            "static const TSSymbol ts_alias_sequences[{}] = {{",
            symbols.len()
        );
        indent!(self);
        add_line!(self, "[0] = 0,");
        for (id, alias) in symbols.into_iter().enumerate() {
            if let Some(alias) = alias {
                add_line!(self, "[{id}] = {},", self.alias_ids[&alias]);
            }
        }
        dedent!(self);
        add_line!(self, "}};\n");
    }

    fn add_non_terminal_alias_map(&mut self) {
        let mut alias_ids_by_symbol = FxHashMap::default();
        for symbol in &self.syntax_grammar.supertype_symbols {
            if self
                .syntax_grammar
                .supertype_alias(*symbol, &self.default_aliases)
                .is_some()
                && let Some(id) = self.symbol_ids.get(symbol)
            {
                alias_ids_by_symbol.insert(*symbol, vec![id]);
            }
        }
        for i in 0..self.syntax_grammar.variables.len() {
            for prod_id in self.syntax_grammar.variable_prod_ids(i) {
                for step in self.syntax_grammar.production(prod_id).steps {
                    if let Some(alias) = step.alias()
                        && step.symbol().is_non_terminal()
                        && Some(alias) != self.default_aliases.get(&step.symbol()).copied()
                        && self.symbol_ids.contains_key(&step.symbol())
                        && let Some(alias_id) = self.alias_ids.get(&alias)
                    {
                        let alias_ids = alias_ids_by_symbol
                            .entry(step.symbol())
                            .or_insert(Vec::new());
                        if let Err(i) = alias_ids.binary_search(&alias_id) {
                            alias_ids.insert(i, alias_id);
                        }
                    }
                }
            }
        }

        let mut alias_ids_by_symbol = alias_ids_by_symbol.iter().collect::<Vec<_>>();
        alias_ids_by_symbol.sort_unstable_by_key(|e| e.0);

        add_line!(
            self,
            "static const uint16_t ts_non_terminal_alias_map[] = {{"
        );
        indent!(self);
        for (symbol, alias_ids) in alias_ids_by_symbol {
            let symbol_id = &self.symbol_ids[symbol];
            let public_symbol_id = &self.symbol_ids[&self.symbol_map[symbol]];
            add_line!(self, "{symbol_id}, {},", 1 + alias_ids.len());
            indent!(self);
            add_line!(self, "{public_symbol_id},");
            for alias_id in alias_ids {
                add_line!(self, "{alias_id},");
            }
            dedent!(self);
        }
        add_line!(self, "0,");
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    /// Produces a list of the "primary state" for every state in the grammar.
    ///
    /// The "primary state" for a given state is the first encountered state that behaves
    /// identically with respect to query analysis. We derive this by keeping track of the `core_id`
    /// for each state and treating the first state with a given `core_id` as primary.
    fn add_primary_state_id_list(&mut self) {
        add_line!(
            self,
            "static const TSStateId ts_primary_state_ids[STATE_COUNT] = {{"
        );
        indent!(self);
        let mut first_state_for_each_core_id = FxHashMap::default();
        for (idx, state) in self.parse_table.states.iter().enumerate() {
            let primary_state = first_state_for_each_core_id
                .entry(state.core_id)
                .or_insert(idx);
            add_line!(self, "[{idx}] = {primary_state},");
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_field_sequences(&mut self) {
        let mut flat_field_maps = IndexMap::with_hasher(FxBuildHasher);
        let mut next_flat_field_map_index = 0;
        flat_field_maps.insert(Vec::new(), 0);

        let mut field_map_ids = Vec::with_capacity(self.parse_table.production_infos.len());
        for production_info in &self.parse_table.production_infos {
            if production_info.field_map.is_empty() {
                field_map_ids.push((0, 0));
            } else {
                let mut flat_field_map = Vec::with_capacity(production_info.field_map.len());
                for (field_name, locations) in &production_info.field_map {
                    for location in locations {
                        flat_field_map.push((*field_name, *location));
                    }
                }
                flat_field_map.sort_by(|(a, _), (b, _)| {
                    self.str_pool.resolve(*a).cmp(self.str_pool.resolve(*b))
                });
                let field_map_len = flat_field_map.len();
                let id = *flat_field_maps.entry(flat_field_map).or_insert_with(|| {
                    let id = next_flat_field_map_index;
                    next_flat_field_map_index += field_map_len;
                    id
                });
                field_map_ids.push((id, field_map_len));
            }
        }

        add_line!(
            self,
            "static const TSMapSlice ts_field_map_slices[PRODUCTION_ID_COUNT] = {{",
        );
        indent!(self);
        for (production_id, (row_id, length)) in field_map_ids.into_iter().enumerate() {
            if length > 0 {
                add_line!(
                    self,
                    "[{production_id}] = {{.index = {row_id}, .length = {length}}},",
                );
            }
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");

        add_line!(
            self,
            "static const TSFieldMapEntry ts_field_map_entries[] = {{",
        );
        indent!(self);
        for (field_pairs, row_index) in flat_field_maps.into_iter().skip(1) {
            add_line!(self, "[{row_index}] =");
            indent!(self);
            for (field_name, location) in field_pairs {
                add_whitespace!(self);
                add!(
                    self,
                    "{{{}, {}",
                    Self::field_id(self.str_pool.resolve(field_name)),
                    location.index
                );
                if location.inherited {
                    add!(self, ", .inherited = true");
                }
                add!(self, "}},\n");
            }
            dedent!(self);
        }

        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_supertype_map(&mut self) {
        add_line!(
            self,
            "static const TSSymbol ts_supertype_symbols[SUPERTYPE_COUNT] = {{"
        );
        indent!(self);
        for supertype in self.supertype_map.keys() {
            add_line!(self, "{supertype},");
        }
        dedent!(self);
        add_line!(self, "}};\n");

        add_line!(
            self,
            "static const TSMapSlice ts_supertype_map_slices[] = {{",
        );
        indent!(self);
        let mut row_id = 0;
        let mut supertype_ids = vec![0];
        let mut supertype_string_map = BTreeMap::new();
        for (supertype, subtypes) in &self.supertype_map {
            supertype_string_map.insert(
                supertype,
                subtypes
                    .iter()
                    .flat_map(|s| match s {
                        ChildType::Normal(symbol) => vec![self.symbol_ids.get(symbol).cloned()],
                        ChildType::Aliased(alias) => {
                            self.alias_ids.get(alias).cloned().map_or_else(
                                || {
                                    self.symbols_for_alias(*alias)
                                        .iter()
                                        .map(|s| self.symbol_ids.get(s).cloned())
                                        .collect()
                                },
                                |a| vec![Some(a)],
                            )
                        }
                    })
                    .flatten()
                    .collect::<BTreeSet<String>>(),
            );
        }
        for (supertype, subtypes) in &supertype_string_map {
            let length = subtypes.len();
            add_line!(
                self,
                "[{supertype}] = {{.index = {row_id}, .length = {length}}},",
            );
            row_id += length;
            supertype_ids.push(row_id);
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");

        add_line!(
            self,
            "static const TSSymbol ts_supertype_map_entries[] = {{",
        );
        indent!(self);
        for (i, (_, subtypes)) in supertype_string_map.iter().enumerate() {
            let row_index = supertype_ids[i];
            add_line!(self, "[{row_index}] =");
            indent!(self);
            for subtype in subtypes {
                add_whitespace!(self);
                add!(self, "{subtype},\n");
            }
            dedent!(self);
        }

        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_lex_function(&mut self, name: &str, lex_table: LexTable) {
        if lex_table.states.len() > MAX_SINGLE_LEXER_STATES {
            self.add_chunked_lex_function(name, &lex_table);
            return;
        }
        add_line!(
            self,
            "static bool {name}(TSLexer *lexer, TSStateId state) {{",
        );
        indent!(self);

        add_line!(self, "START_LEXER();");
        add_line!(self, "eof = lookahead == 0 && lexer->eof(lexer);");
        add_line!(self, "switch (state) {{");

        indent!(self);
        for (i, state) in lex_table.states.into_iter().enumerate() {
            add_line!(self, "case {i}:");
            indent!(self);
            self.add_lex_state(i as u32, state);
            dedent!(self);
        }

        add_line!(self, "default:");
        indent!(self);
        add_line!(self, "return false;");
        dedent!(self);

        dedent!(self);
        add_line!(self, "}}");

        dedent!(self);
        add_line!(self, "}}");
        add_line!(self, "");
    }

    fn add_chunked_lex_function(&mut self, name: &str, lex_table: &LexTable) {
        let chunk_count = lex_table.states.len().div_ceil(LEXER_CHUNK_SIZE);
        for chunk in 0..chunk_count {
            add_line!(
                self,
                "static uint8_t {name}_{chunk}(TSLexer *, TSStateId *, bool *);"
            );
        }
        add_line!(self, "");
        add_line!(
            self,
            "static bool {name}(TSLexer *lexer, TSStateId state) {{"
        );
        indent!(self);
        add_line!(self, "bool result = false;");
        add_line!(self, "for (;;) {{");
        indent!(self);
        add_line!(self, "uint8_t status;");
        add_line!(self, "switch (state / {LEXER_CHUNK_SIZE}) {{");
        indent!(self);
        for chunk in 0..chunk_count {
            add_line!(
                self,
                "case {chunk}: status = {name}_{chunk}(lexer, &state, &result); break;"
            );
        }
        add_line!(self, "default: return result;");
        dedent!(self);
        add_line!(self, "}}");
        add_line!(self, "if (status < 2) return status != 0;");
        dedent!(self);
        add_line!(self, "}}");
        dedent!(self);
        add_line!(self, "}}\n");

        for (chunk, states) in lex_table.states.chunks(LEXER_CHUNK_SIZE).enumerate() {
            let has_advances = states.iter().enumerate().any(|(offset, state)| {
                let id = (chunk * LEXER_CHUNK_SIZE + offset) as u32;
                state.eof_action.is_some()
                    || state
                        .advance_actions
                        .iter()
                        .any(|(_, action)| action.state != id)
            });
            add_line!(
                self,
                "static uint8_t {name}_{chunk}(TSLexer *lexer, TSStateId *state_out, bool *result_out) {{"
            );
            indent!(self);
            add_line!(self, "TSStateId state = *state_out;");
            add_line!(self, "bool result = *result_out;");
            if has_advances {
                add_line!(self, "bool skip = false;");
            }
            add_line!(self, "UNUSED bool eof;");
            add_line!(self, "int32_t lookahead;");
            if has_advances {
                add_line!(self, "goto start;");
                add_line!(self, "next_state:");
                add_line!(self, "lexer->advance(lexer, skip);");
                add_line!(self, "start:");
                add_line!(self, "skip = false;");
            }
            add_line!(self, "lookahead = lexer->lookahead;");
            add_line!(self, "eof = lookahead == 0 && lexer->eof(lexer);");
            add_line!(self, "switch (state) {{");
            indent!(self);
            for (offset, state) in states.iter().enumerate() {
                let id = (chunk * LEXER_CHUNK_SIZE + offset) as u32;
                add_line!(self, "case {id}:");
                indent!(self);
                self.add_lex_state(id, state.clone());
                dedent!(self);
            }
            add_line!(self, "default:");
            indent!(self);
            add_line!(self, "*state_out = state;");
            add_line!(self, "*result_out = result;");
            add_line!(self, "return 2;");
            dedent!(self);
            dedent!(self);
            add_line!(self, "}}");
            dedent!(self);
            add_line!(self, "}}\n");
        }
    }

    fn add_lex_state(&mut self, state_id: u32, state: LexState) {
        if state
            .advance_actions
            .iter()
            .any(|(_, action)| action.state == state_id)
        {
            add_line!(self, "ts_lex_state_{state_id}:");
        }
        let defer_accept = state.eof_action.is_none()
            && !state.advance_actions.is_empty()
            && state
                .advance_actions
                .iter()
                .all(|(_, action)| action.state == state_id && action.in_main_token);
        if !defer_accept && let Some(accept_action) = state.accept_action {
            add_line!(self, "ACCEPT_TOKEN({});", self.symbol_ids[&accept_action]);
        }

        if let Some(eof_action) = state.eof_action {
            add_line!(self, "if (eof) ADVANCE({});", eof_action.state);
        }

        let mut chars_copy = CharacterSet::empty();
        let mut large_set = CharacterSet::empty();
        let mut ruled_out_chars = CharacterSet::empty();

        let mut mapped_transitions = vec![false; state.advance_actions.len()];
        let mut simple_transition_range_count = 0;
        for (index, (chars, action)) in state.advance_actions.iter().enumerate() {
            // The map compares the lookahead alone, which is also 0 at the end of the input.
            if action.in_main_token
                && action.state != state_id
                && !chars.contains('\0')
                && chars.ranges().all(|r| {
                    let start = *r.start() as u32;
                    let end = *r.end() as u32;
                    end <= start + 1 && u16::try_from(end).is_ok()
                })
            {
                mapped_transitions[index] = true;
                simple_transition_range_count += chars.range_count();
            }
        }

        if simple_transition_range_count >= 8 {
            let mut transitions = Vec::new();
            for (index, (chars, action)) in state.advance_actions.iter().enumerate() {
                if !mapped_transitions[index] {
                    continue;
                }
                for range in chars.ranges() {
                    transitions.push((*range.start(), action.state));
                    if range.end() > range.start() {
                        transitions.push((*range.end(), action.state));
                    }
                }
                ruled_out_chars = ruled_out_chars.add(chars);
            }
            transitions.sort_unstable();
            self.add_advance_map(&transitions);
        } else {
            mapped_transitions.fill(false);
        }

        for (index, (chars, action)) in state.advance_actions.iter().enumerate() {
            if mapped_transitions[index] {
                continue;
            }
            add_whitespace!(self);

            // The lex state's advance actions are represented with disjoint
            // sets of characters. When translating these disjoint sets into a
            // sequence of checks, we don't need to re-check conditions that
            // have already been checked due to previous transitions.
            //
            // Note that this simplification may result in an empty character set.
            // That means that the transition is guaranteed (nothing further needs to
            // be checked), not that this transition is impossible.
            let simplified_chars = chars.simplify_ignoring(&ruled_out_chars);

            // For large character sets, find the best matching character set from
            // a pre-selected list of large character sets, which are based on the
            // state transitions for individual tokens. This transition may not exactly
            // match one of the pre-selected character sets. In that case, determine
            // the additional checks that need to be performed to match this transition.
            let mut best_large_char_set: Option<(usize, CharacterSet, CharacterSet)> = None;
            if simplified_chars.range_count() >= super::build_tables::LARGE_CHARACTER_RANGE_COUNT {
                // Prefer the last candidate on ties. Searching backwards lets
                // us stop as soon as a match needs no additional checks.
                for (ix, (_, set)) in self.large_character_sets.iter().enumerate().rev() {
                    chars_copy.assign(&simplified_chars);
                    large_set.assign(set);
                    let intersection = chars_copy.remove_intersection(&mut large_set);
                    if !intersection.is_empty() {
                        let additions = chars_copy.simplify_ignoring(&ruled_out_chars);
                        let removals = large_set.simplify_ignoring(&ruled_out_chars);
                        let total_range_count = additions.range_count() + removals.range_count();
                        if total_range_count >= simplified_chars.range_count() {
                            continue;
                        }
                        if let Some((_, best_additions, best_removals)) = &best_large_char_set {
                            let best_range_count =
                                best_additions.range_count() + best_removals.range_count();
                            // Only a set that needs fewer checks replaces the best one, so on
                            // a tie the set found first wins.
                            if best_range_count <= total_range_count {
                                continue;
                            }
                        }
                        best_large_char_set = Some((ix, additions, removals));
                        if total_range_count == 0 {
                            break;
                        }
                    }
                }
            }

            // Add this transition's character set to the set of ruled out characters,
            // which don't need to be checked for subsequent transitions in this state.
            ruled_out_chars = ruled_out_chars.add(chars);

            let mut large_char_set_ix = None;
            let mut asserted_chars = simplified_chars;
            let mut negated_chars = CharacterSet::empty();
            if let Some((char_set_ix, additions, removals)) = best_large_char_set {
                asserted_chars = additions;
                negated_chars = removals;
                large_char_set_ix = Some(char_set_ix);
            }

            let line_break = format!("\n{}", "  ".repeat(self.indent_level + 2));

            let has_positive_condition = large_char_set_ix.is_some() || !asserted_chars.is_empty();
            let has_negative_condition = !negated_chars.is_empty();
            let has_condition = has_positive_condition || has_negative_condition;
            if has_condition {
                add!(self, "if (");
                if has_positive_condition && has_negative_condition {
                    add!(self, "(");
                }
            }

            if let Some(large_char_set_ix) = large_char_set_ix {
                let large_set = &self.large_character_sets[large_char_set_ix].1;

                // If the character set contains the null character, check that we
                // are not at the end of the file.
                let check_eof = large_set.contains('\0');
                if check_eof {
                    add!(self, "(!eof && ");
                }

                let char_set_info = &mut self.large_character_set_info[large_char_set_ix];
                char_set_info.is_used = true;
                if let Some(pages) = &char_set_info.unicode_pages {
                    let width = if pages.blocks.len() <= 256 { 8 } else { 16 };
                    add!(
                        self,
                        "ts_lex_pages{width}_contains({0}_ascii, {0}_page_ids, {0}_pages, {1}, {2}, lookahead)",
                        char_set_info.constant_name,
                        pages.first_page,
                        pages.page_ids.len()
                    );
                } else {
                    add!(
                        self,
                        "ts_lex_set_contains_with_ascii({}_ascii, {}, {}, lookahead)",
                        char_set_info.constant_name,
                        char_set_info.constant_name,
                        large_set.ranges().filter(|r| *r.end() >= '\u{80}').count(),
                    );
                }
                if check_eof {
                    add!(self, ")");
                }
            }

            if !asserted_chars.is_empty() {
                if large_char_set_ix.is_some() {
                    add!(self, " ||{line_break}");
                }

                // If the character set contains the max character, then it probably
                // corresponds to a negated character class in a regex, so it will be more
                // concise and readable to express it in terms of negated ranges.
                let is_included = !asserted_chars.contains(char::MAX);
                if is_included {
                    if asserted_chars.range_count() >= 4
                        && asserted_chars.ranges().all(|range| range.end().is_ascii())
                    {
                        let mut ascii = [0u32; 4];
                        for range in asserted_chars.ranges() {
                            for character in *range.start() as usize..=*range.end() as usize {
                                ascii[character / 32] |= 1 << (character % 32);
                            }
                        }
                        let next_id = self.ascii_sets.len();
                        let id = *self.ascii_sets.entry(ascii).or_insert(next_id);
                        if asserted_chars.contains('\0') {
                            add!(self, "(!eof && ");
                        }
                        add!(
                            self,
                            "ts_lex_ascii_contains(ts_lex_ascii_set_{id}, lookahead)"
                        );
                        if asserted_chars.contains('\0') {
                            add!(self, ")");
                        }
                    } else {
                        self.add_character_range_conditions(&asserted_chars, true, &line_break);
                    }
                } else {
                    let excluded_chars = asserted_chars.negate();
                    // The lookahead is also 0 at the end of the input, so a set containing NUL
                    // must check for it explicitly.
                    let check_eof = !excluded_chars.contains('\0');
                    // Parenthesized after `||` to keep C compilers' `-Wall` from warning about
                    // `&&` within `||`.
                    let wrap = large_char_set_ix.is_some();
                    if wrap {
                        add!(self, "(");
                    }
                    if check_eof {
                        add!(self, "!eof");
                        if !excluded_chars.is_empty() {
                            add!(self, " &&{line_break}");
                        }
                    }
                    self.add_character_range_conditions(&excluded_chars, false, &line_break);
                    if wrap {
                        add!(self, ")");
                    }
                }
            }

            if has_negative_condition {
                if has_positive_condition {
                    add!(self, ") &&{line_break}");
                }
                self.add_character_range_conditions(&negated_chars, false, &line_break);
            }

            if has_condition {
                add!(self, ") ");
            }

            self.add_advance_action(action, state_id);
            add!(self, "\n");
        }

        if defer_accept && let Some(accept_action) = state.accept_action {
            add_line!(self, "ACCEPT_TOKEN({});", self.symbol_ids[&accept_action]);
        }
        add_line!(self, "END_STATE();");
    }

    fn add_advance_map(&mut self, transitions: &[(char, u32)]) {
        let next_id = self.advance_maps.len();
        let id = *self
            .advance_maps
            .entry(transitions.to_vec())
            .or_insert(next_id);
        let (first, _, dense) = Self::advance_map_layout(transitions);
        if dense {
            add_line!(
                self,
                "TS_LEX_ADVANCE_MAP_DENSE({first}, ts_lex_advance_map_{id});"
            );
        } else {
            add_line!(self, "TS_LEX_ADVANCE_MAP_SORTED(ts_lex_advance_map_{id});");
        }
    }

    fn advance_map_layout(transitions: &[(char, u32)]) -> (u32, u32, bool) {
        let first = transitions[0].0 as u32;
        let span = transitions.last().unwrap().0 as u32 - first + 1;
        let dense = span <= 128
            && span as usize <= transitions.len() * 2
            && transitions
                .iter()
                .all(|(_, state)| *state < u32::from(u16::MAX));
        (first, span, dense)
    }

    fn add_character_range_conditions(
        &mut self,
        characters: &CharacterSet,
        is_included: bool,
        line_break: &str,
    ) {
        for (i, range) in characters.ranges().enumerate() {
            let start = *range.start();
            let end = *range.end();
            if is_included {
                if i > 0 {
                    add!(self, " ||{line_break}");
                }

                if start == '\0' {
                    add!(self, "(!eof && ");
                    if end == '\0' {
                        add!(self, "lookahead == ");
                    } else {
                        add!(self, "lookahead <= ");
                    }
                    self.add_character(end);
                    add!(self, ")");
                } else if end == start {
                    add!(self, "lookahead == ");
                    self.add_character(start);
                } else if end as u32 == start as u32 + 1 {
                    add!(self, "lookahead == ");
                    self.add_character(start);
                    add!(self, " ||{line_break}lookahead == ");
                    self.add_character(end);
                } else {
                    add!(self, "(");
                    self.add_character(start);
                    add!(self, " <= lookahead && lookahead <= ");
                    self.add_character(end);
                    add!(self, ")");
                }
            } else {
                if i > 0 {
                    add!(self, " &&{line_break}");
                }
                if end == start {
                    add!(self, "lookahead != ");
                    self.add_character(start);
                } else if end as u32 == start as u32 + 1 {
                    add!(self, "lookahead != ");
                    self.add_character(start);
                    add!(self, " &&{line_break}lookahead != ");
                    self.add_character(end);
                } else if start != '\0' {
                    add!(self, "(lookahead < ");
                    self.add_character(start);
                    add!(self, " || ");
                    self.add_character(end);
                    add!(self, " < lookahead)");
                } else {
                    add!(self, "lookahead > ");
                    self.add_character(end);
                }
            }
        }
    }

    fn add_character_set(&mut self, ix: usize) {
        let characters = self.large_character_sets[ix].1.clone();
        let info = &self.large_character_set_info[ix];
        if !info.is_used {
            return;
        }

        let mut ascii = [0u32; 4];
        for character in 0..128 {
            if characters.contains(char::from_u32(character).unwrap()) {
                ascii[character as usize / 32] |= 1 << (character % 32);
            }
        }
        add_line!(
            self,
            "static const uint32_t {}_ascii[] = {{0x{:08x}, 0x{:08x}, 0x{:08x}, 0x{:08x}}};",
            info.constant_name,
            ascii[0],
            ascii[1],
            ascii[2],
            ascii[3],
        );

        if let Some(pages) = &info.unicode_pages {
            let width = if pages.blocks.len() <= 256 { 8 } else { 16 };
            add_line!(
                self,
                "static const uint{width}_t {}_page_ids[] = {{",
                info.constant_name
            );
            indent!(self);
            for ids in pages.page_ids.chunks(16) {
                add_whitespace!(self);
                for id in ids {
                    add!(self, "{id}, ");
                }
                add!(self, "\n");
            }
            dedent!(self);
            add_line!(self, "}};");
            add_line!(
                self,
                "static const uint32_t {}_pages[][8] = {{",
                info.constant_name
            );
            indent!(self);
            for block in &pages.blocks {
                add_whitespace!(self);
                add!(self, "{{");
                for word in block {
                    add!(self, "0x{word:08x}, ");
                }
                add!(self, "}},\n");
            }
            dedent!(self);
            add_line!(self, "}};\n");
            return;
        }

        add_line!(
            self,
            "static const TSCharacterRange {}[] = {{",
            info.constant_name
        );

        indent!(self);
        let unicode_ranges = characters
            .ranges()
            .filter(|range| *range.end() >= '\u{80}')
            .collect::<Vec<_>>();
        if unicode_ranges.is_empty() {
            add_line!(self, "{{0, 0}},");
        }
        for (ix, range) in unicode_ranges.into_iter().enumerate() {
            let column = ix % 8;
            if column == 0 {
                if ix > 0 {
                    add!(self, "\n");
                }
                add_whitespace!(self);
            } else {
                add!(self, " ");
            }
            add!(self, "{{");
            self.add_character(cmp::max(*range.start(), '\u{80}'));
            add!(self, ", ");
            self.add_character(*range.end());
            add!(self, "}},");
        }
        add!(self, "\n");
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn unicode_pages(characters: &CharacterSet) -> Option<UnicodePages> {
        let ranges = characters
            .ranges()
            .filter(|range| *range.end() >= '\u{80}')
            .map(|range| ((*range.start() as u32).max(128), *range.end() as u32))
            .collect::<Vec<_>>();
        let first_page = ranges.first()?.0 / 256;
        let page_count = (ranges.last()?.1 / 256 - first_page + 1) as usize;
        let range_bytes = ranges.len() * 2 * size_of::<u32>();
        if page_count + 32 >= range_bytes {
            return None;
        }
        let mut pages = vec![[0u32; 8]; page_count];
        for (start, end) in ranges {
            for character in start..=end {
                pages[(character / 256 - first_page) as usize][(character % 256 / 32) as usize] |=
                    1 << (character % 32);
            }
        }
        let mut blocks = IndexMap::<[u32; 8], u16, FxBuildHasher>::default();
        let page_ids = pages
            .into_iter()
            .map(|page| {
                let next_id = blocks.len() as u16;
                *blocks.entry(page).or_insert(next_id)
            })
            .collect::<Vec<_>>();
        let index_bytes = page_ids.len() * if blocks.len() <= 256 { 1 } else { 2 };
        if index_bytes + blocks.len() * 32 + 32 >= range_bytes {
            return None;
        }
        Some(UnicodePages {
            first_page,
            page_ids,
            blocks: blocks.into_keys().collect(),
        })
    }

    fn add_advance_action(&mut self, action: &AdvanceAction, state_id: u32) {
        if action.state == state_id {
            self.uses_lex_repeat = true;
            add!(
                self,
                "TS_LEX_REPEAT({}, ts_lex_state_{state_id});",
                !action.in_main_token
            );
        } else if action.in_main_token {
            add!(self, "ADVANCE({});", action.state);
        } else {
            add!(self, "SKIP({});", action.state);
        }
    }

    fn add_lex_modes(&mut self) {
        if self.abi_version >= ABI_VERSION_WITH_COMPACT_TABLES && self.add_shared_lex_modes() {
            return;
        }

        add_line!(
            self,
            "static const {} ts_lex_modes[STATE_COUNT] = {{",
            if self.abi_version >= ABI_VERSION_WITH_RESERVED_WORDS {
                "TSLexerMode"
            } else {
                "TSLexMode"
            }
        );
        indent!(self);
        for (i, state) in self.parse_table.states.iter().enumerate() {
            add_whitespace!(self);
            add!(self, "[{i}] = {{");
            if state.is_end_of_non_terminal_extra() {
                add!(self, "(TSStateId)(-1),");
            } else {
                add!(self, ".lex_state = {}", state.lex_state_id);

                if state.external_lex_state_id > 0 {
                    add!(
                        self,
                        ", .external_lex_state = {}",
                        state.external_lex_state_id
                    );
                }

                if self.abi_version >= ABI_VERSION_WITH_RESERVED_WORDS {
                    let reserved_word_set_id = self.reserved_word_set_ids_by_parse_state[i];
                    if reserved_word_set_id != 0 {
                        add!(self, ", .reserved_word_set_id = {reserved_word_set_id}");
                    }
                }
            }

            add!(self, "}},\n");
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_shared_lex_modes(&mut self) -> bool {
        let mut modes = IndexMap::<(u32, u32, usize), usize, FxBuildHasher>::default();
        let mut ids = Vec::new();
        for (id, state) in self.parse_table.states.iter().enumerate() {
            let mode = if state.is_end_of_non_terminal_extra() {
                (u32::from(u16::MAX), 0, 0)
            } else {
                (
                    state.lex_state_id,
                    state.external_lex_state_id,
                    self.reserved_word_set_ids_by_parse_state[id],
                )
            };
            let next = modes.len();
            ids.push(*modes.entry(mode).or_insert(next));
        }
        if modes.len() * 6 + ids.len() * 2 >= ids.len() * 6 {
            return false;
        }
        self.lex_mode_count = modes.len();
        add_line!(self, "static const TSLexerMode ts_lex_modes[] = {{");
        indent!(self);
        for (&(lex, external, reserved), &id) in &modes {
            add_line!(
                self,
                "[{id}] = {{.lex_state = {lex}, .external_lex_state = {external}, .reserved_word_set_id = {reserved}}},"
            );
        }
        dedent!(self);
        add_line!(self, "}};\n");
        add_line!(
            self,
            "static const uint16_t ts_lex_mode_ids[STATE_COUNT] = {{"
        );
        indent!(self);
        for chunk in ids.chunks(16) {
            add_whitespace!(self);
            for id in chunk {
                add!(self, "{id}, ");
            }
            add!(self, "\n");
        }
        dedent!(self);
        add_line!(self, "}};\n");
        true
    }

    fn add_reserved_word_sets(&mut self) {
        let total = self
            .reserved_word_sets
            .iter()
            .map(TokenSet::len)
            .sum::<usize>();
        let maximum = self
            .reserved_word_sets
            .iter()
            .map(TokenSet::len)
            .max()
            .unwrap_or(0);
        if self.abi_version >= ABI_VERSION_WITH_COMPACT_TABLES
            && u16::try_from(total).is_ok()
            && total * 2 + self.reserved_word_sets.len() * 4
                < maximum * self.reserved_word_sets.len() * 2
        {
            self.uses_reserved_word_slices = true;
            add_line!(self, "static const TSSymbol ts_reserved_words[] = {{");
            indent!(self);
            for set in &self.reserved_word_sets {
                for token in set.iter() {
                    add_line!(self, "{},", self.symbol_ids[&token]);
                }
            }
            dedent!(self);
            add_line!(self, "}};");
            add_line!(
                self,
                "static const TSMapSlice ts_reserved_word_slices[] = {{"
            );
            indent!(self);
            let mut offset = 0;
            for set in &self.reserved_word_sets {
                add_line!(self, "{{.index = {offset}, .length = {}}},", set.len());
                offset += set.len();
            }
            dedent!(self);
            add_line!(self, "}};");
            return;
        }
        add_line!(
            self,
            "static const TSSymbol ts_reserved_words[{}][MAX_RESERVED_WORD_SET_SIZE] = {{",
            self.reserved_word_sets.len(),
        );
        indent!(self);
        for (id, set) in self.reserved_word_sets.iter().enumerate() {
            if id == 0 {
                continue;
            }
            add_line!(self, "[{id}] = {{");
            indent!(self);
            for token in set.iter() {
                add_line!(self, "{},", self.symbol_ids[&token]);
            }
            dedent!(self);
            add_line!(self, "}},");
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_external_token_enum(&mut self) {
        add_line!(self, "enum ts_external_scanner_symbol_identifiers {{");
        indent!(self);
        for i in 0..self.syntax_grammar.external_tokens.len() {
            add_line!(self, "{} = {i},", self.external_token_id(i));
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_external_scanner_symbol_map(&mut self) {
        add_line!(
            self,
            "static const TSSymbol ts_external_scanner_symbol_map[EXTERNAL_TOKEN_COUNT] = {{"
        );
        indent!(self);
        for i in 0..self.syntax_grammar.external_tokens.len() {
            let token = &self.syntax_grammar.external_tokens[i];
            let id_token = token
                .corresponding_internal_token
                .unwrap_or_else(|| Symbol::external(i));
            add_line!(
                self,
                "[{}] = {},",
                self.external_token_id(i),
                self.symbol_ids[&id_token],
            );
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_external_scanner_states_list(&mut self) {
        let tokens = self.syntax_grammar.external_tokens.len();
        let stride = tokens.div_ceil(8);
        if self.abi_version >= ABI_VERSION_WITH_COMPACT_TABLES
            && self.profile.is_none()
            && self.parse_table.external_lex_states.len() * (tokens - stride) > 128
        {
            self.external_state_stride = stride;
            add_line!(
                self,
                "static const uint8_t ts_external_scanner_states[{}][{stride}] = {{",
                self.parse_table.external_lex_states.len()
            );
            indent!(self);
            for (i, state) in self.parse_table.external_lex_states.iter().enumerate() {
                let mut bytes = vec![0u8; stride];
                for token in state.iter() {
                    bytes[token.index as usize / 8] |= 1 << (token.index as usize % 8);
                }
                add_line!(
                    self,
                    "[{i}] = {{ {} }},",
                    bytes
                        .iter()
                        .map(u8::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            dedent!(self);
            add_line!(self, "}};");
            return;
        }
        add_line!(
            self,
            "static const bool ts_external_scanner_states[{}][EXTERNAL_TOKEN_COUNT] = {{",
            self.parse_table.external_lex_states.len(),
        );
        indent!(self);
        for i in 0..self.parse_table.external_lex_states.len() {
            if !self.parse_table.external_lex_states[i].is_empty() {
                add_line!(self, "[{i}] = {{");
                indent!(self);
                for token in self.parse_table.external_lex_states[i].iter() {
                    add_line!(
                        self,
                        "[{}] = true,",
                        self.external_token_id(token.index as usize)
                    );
                }
                dedent!(self);
                add_line!(self, "}},");
            }
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_parse_table(&mut self) -> RenderResult<()> {
        let mut parse_table_entries = FxHashMap::default();
        let mut next_parse_action_list_index = 0u32;

        // Parse action lists zero is for the default value, when a symbol is not valid.
        // `canonicalize` guarantees pool index 0 is the empty list.
        Self::get_parse_action_list_id(
            ActionListId::new(0, false),
            &self.parse_table.action_lists,
            &mut parse_table_entries,
            &mut next_parse_action_list_index,
        );

        add_line!(
            self,
            "static const uint16_t ts_parse_table[LARGE_STATE_COUNT][SYMBOL_COUNT] = {{",
        );
        indent!(self);

        let mut terminal_entries = Vec::new();
        let mut nonterminal_entries = Vec::new();

        for (i, state) in self
            .parse_table
            .states
            .iter()
            .enumerate()
            .take(self.large_state_count)
        {
            add_line!(self, "[STATE({i})] = {{");
            indent!(self);

            terminal_entries.clear();
            nonterminal_entries.clear();
            terminal_entries.extend(state.terminal_entries.iter());
            nonterminal_entries.extend(state.nonterminal_entries.iter());
            terminal_entries.sort_unstable_by_key(|e| self.symbol_order.get(e.0));
            nonterminal_entries.sort_unstable_by_key(|k| k.0);

            for (symbol, action) in &nonterminal_entries {
                add_line!(
                    self,
                    "[{}] = STATE({}),",
                    self.symbol_ids[symbol],
                    match action {
                        GotoAction::Goto(state) => *state as usize,
                        GotoAction::ShiftExtra => i,
                    }
                );
            }

            for (symbol, id) in &terminal_entries {
                let entry_id = Self::get_parse_action_list_id(
                    **id,
                    &self.parse_table.action_lists,
                    &mut parse_table_entries,
                    &mut next_parse_action_list_index,
                );
                add_line!(self, "[{}] = ACTIONS({entry_id}),", self.symbol_ids[symbol]);
            }

            dedent!(self);
            add_line!(self, "}},");
        }

        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");

        if self.large_state_count < self.parse_table.states.len() {
            add_line!(self, "static const uint16_t ts_small_parse_table[] = {{");
            indent!(self);

            let mut next_table_index = 0;
            let mut small_state_indices = Vec::with_capacity(
                self.parse_table
                    .states
                    .len()
                    .saturating_sub(self.large_state_count),
            );
            let mut symbols_by_value = FxHashMap::<(u32, SymbolType), Vec<Symbol>>::default();
            let mut row_offsets = FxHashMap::default();
            let has_profile_samples = self
                .profile
                .as_ref()
                .is_some_and(|profile| profile.parse_states.iter().any(|&count| count > 0));
            for (state_id, state) in self
                .parse_table
                .states
                .iter()
                .enumerate()
                .skip(self.large_state_count)
            {
                symbols_by_value.clear();

                terminal_entries.clear();
                terminal_entries.extend(state.terminal_entries.iter());
                terminal_entries.sort_unstable_by_key(|e| self.symbol_order.get(e.0));

                for (symbol, entry) in &terminal_entries {
                    let entry_id = Self::get_parse_action_list_id(
                        **entry,
                        &self.parse_table.action_lists,
                        &mut parse_table_entries,
                        &mut next_parse_action_list_index,
                    );
                    symbols_by_value
                        .entry((entry_id, SymbolType::Terminal))
                        .or_default()
                        .push(**symbol);
                }
                for (symbol, action) in state.nonterminal_entries.iter() {
                    let state_id = self.small_goto_value(*action, state_id);
                    symbols_by_value
                        .entry((state_id, SymbolType::NonTerminal))
                        .or_default()
                        .push(*symbol);
                }

                let mut values_with_symbols = symbols_by_value.drain().collect::<Vec<_>>();
                values_with_symbols.sort_unstable_by_key(|((value, kind), symbols)| {
                    (symbols.len(), *kind, *value, symbols[0])
                });

                let mut row = vec![values_with_symbols.len() as u32];
                for ((value, _), symbols) in &mut values_with_symbols {
                    if self.abi_version >= ABI_VERSION_WITH_COMPACT_TABLES {
                        symbols.sort_unstable_by_key(|symbol| self.symbol_order[symbol]);
                    } else {
                        symbols.sort_unstable();
                    }
                    row.extend([*value, symbols.len() as u32]);
                    row.extend(
                        symbols
                            .iter()
                            .map(|symbol| self.symbol_order[symbol] as u32),
                    );
                }
                // Observed states come first in profile order; shared rows retain their encoding.
                if let Some(&offset) = row_offsets.get(&row) {
                    small_state_indices.push(offset);
                    continue;
                }
                let row_key = row;
                let entry_count = values_with_symbols
                    .iter()
                    .map(|(_, symbols)| symbols.len())
                    .sum::<usize>();
                let minimize = has_profile_samples
                    && self
                        .profile
                        .as_ref()
                        .is_some_and(|profile| profile.parse_states[state_id] == 0);
                let (flag, _) =
                    self.small_state_encoding(entry_count, values_with_symbols.len(), minimize);
                let bitmap = flag == SMALL_STATE_BITMAP_FLAG;
                let pairs = flag == SMALL_STATE_PAIR_FLAG;
                let mut entries = Vec::new();
                let mut row = Vec::<u32>::new();
                if pairs || bitmap {
                    for ((value, kind), symbols) in &values_with_symbols {
                        entries.extend(symbols.iter().map(|symbol| (*symbol, *value, *kind)));
                    }
                    entries.sort_unstable_by_key(|(symbol, _, _)| self.symbol_order[symbol]);
                    if bitmap {
                        let blocks = self.parse_table.symbols.len().div_ceil(16);
                        row.resize(blocks * 2, 0);
                        for (symbol, _, _) in &entries {
                            let symbol = self.symbol_order[symbol];
                            row[(symbol / 16) * 2] |= 1 << (symbol % 16);
                        }
                        let mut rank = 0;
                        for block in 0..blocks {
                            row[block * 2 + 1] = rank;
                            rank += row[block * 2].count_ones();
                        }
                        row.extend(entries.iter().map(|(_, value, _)| *value));
                    }
                }
                if self.abi_version >= ABI_VERSION_WITH_COMPACT_TABLES
                    && next_table_index >= SMALL_STATE_BITMAP_FLAG
                {
                    return Err(RenderError::SparseTable(next_table_index));
                }
                let offset = next_table_index | flag;
                small_state_indices.push(offset);
                row_offsets.insert(row_key, offset);
                if bitmap {
                    add_line!(self, "[{next_table_index}] =");
                    indent!(self);
                    for chunk in row.chunks(16) {
                        for value in chunk {
                            add!(self, "{value}, ");
                        }
                        add_line!(self, "");
                    }
                    dedent!(self);
                    next_table_index += row.len();
                    continue;
                }
                if pairs {
                    add_line!(self, "[{next_table_index}] = {entry_count},");
                    indent!(self);
                    next_table_index += 1;
                    for (symbol, value, kind) in entries {
                        let macro_name = if kind == SymbolType::Terminal {
                            "ACTIONS"
                        } else {
                            "STATE"
                        };
                        add_line!(self, "{}, {macro_name}({value}),", self.symbol_ids[&symbol]);
                        next_table_index += 2;
                    }
                    dedent!(self);
                    continue;
                }

                add_line!(
                    self,
                    "[{next_table_index}] = {},",
                    values_with_symbols.len()
                );
                indent!(self);
                next_table_index += 1;

                for ((value, kind), symbols) in &mut values_with_symbols {
                    next_table_index += 2 + symbols.len();
                    if *kind == SymbolType::NonTerminal {
                        add_line!(self, "STATE({value}), {},", symbols.len());
                    } else {
                        add_line!(self, "ACTIONS({value}), {},", symbols.len());
                    }

                    indent!(self);
                    for symbol in symbols {
                        add_line!(self, "{},", self.symbol_ids[symbol]);
                    }
                    dedent!(self);
                }

                dedent!(self);
            }

            dedent!(self);
            add_line!(self, "}};");
            add_line!(self, "");

            add_line!(
                self,
                "static const uint32_t ts_small_parse_table_map[] = {{"
            );
            indent!(self);
            for i in self.large_state_count..self.parse_table.states.len() {
                add_line!(
                    self,
                    "[SMALL_STATE({i})] = {},",
                    small_state_indices[i - self.large_state_count]
                );
            }
            dedent!(self);
            add_line!(self, "}};");
            add_line!(self, "");
        }
        if next_parse_action_list_index >= u32::from(u16::MAX) {
            Err(RenderError::ParseTable(
                next_parse_action_list_index as usize,
            ))?;
        }

        let mut parse_table_entries = parse_table_entries
            .into_iter()
            .map(|(id, i)| (i, id))
            .collect::<Vec<_>>();
        parse_table_entries.sort_by_key(|(index, _)| *index);
        self.add_parse_action_list(parse_table_entries);

        Ok(())
    }

    fn small_state_encoding(
        &self,
        entry_count: usize,
        group_count: usize,
        minimize: bool,
    ) -> (usize, usize) {
        let grouped = (0, 1 + 2 * group_count + entry_count);
        if self.abi_version < ABI_VERSION_WITH_COMPACT_TABLES {
            return grouped;
        }
        let bitmap = (
            SMALL_STATE_BITMAP_FLAG,
            2 * self.parse_table.symbols.len().div_ceil(16) + entry_count,
        );
        let pairs = (SMALL_STATE_PAIR_FLAG, 1 + 2 * entry_count);
        if minimize {
            [bitmap, pairs, grouped]
                .into_iter()
                .min_by_key(|&(_, words)| words)
                .unwrap()
        } else if entry_count >= BITMAP_STATE_MIN_ENTRIES {
            bitmap
        } else if group_count >= 8 || entry_count <= 2 * group_count {
            pairs
        } else {
            grouped
        }
    }

    fn small_goto_value(&self, action: GotoAction, state_id: usize) -> u32 {
        let destination = match action {
            GotoAction::Goto(id) => id,
            GotoAction::ShiftExtra => state_id as u32,
        };
        if self.abi_version >= ABI_VERSION_WITH_COMPACT_TABLES
            && u16::try_from(self.parse_table.states.len()).is_ok()
            && destination == state_id as u32
        {
            u32::from(u16::MAX)
        } else {
            destination
        }
    }

    fn add_parse_action_list(&mut self, parse_table_entries: Vec<(u32, ActionListId)>) {
        add_line!(
            self,
            "static const TSParseActionEntry ts_parse_actions[] = {{"
        );
        indent!(self);
        for (i, id) in parse_table_entries {
            let actions = self.parse_table.action_lists.get(id);
            add!(
                self,
                "  [{i}] = {{.entry = {{.count = {}, .reusable = {}}}}},",
                actions.len(),
                id.reusable(),
            );
            for action in actions {
                add!(self, " ");
                match *action {
                    ParseAction::Accept => add!(self, " ACCEPT_INPUT()"),
                    ParseAction::Recover => add!(self, "RECOVER()"),
                    ParseAction::ShiftExtra => add!(self, "SHIFT_EXTRA()"),
                    ParseAction::Shift {
                        state,
                        is_repetition,
                    } => {
                        if is_repetition {
                            add!(self, "SHIFT_REPEAT({state})");
                        } else {
                            add!(self, "SHIFT({state})");
                        }
                    }
                    ParseAction::Reduce {
                        symbol,
                        child_count,
                        dynamic_precedence,
                        production_id,
                        ..
                    } => {
                        add!(
                            self,
                            "REDUCE({}, {child_count}, {dynamic_precedence}, {production_id})",
                            self.symbol_ids[&symbol]
                        );
                    }
                }
                add!(self, ",");
            }
            add!(self, "\n");
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_parser_export(&mut self) {
        let language_function_name = format!("tree_sitter_{}", self.language_name);
        let external_scanner_name = format!("{language_function_name}_external_scanner");

        add_line!(self, "#ifdef __cplusplus");
        add_line!(self, r#"extern "C" {{"#);
        add_line!(self, "#endif");

        if !self.syntax_grammar.external_tokens.is_empty() {
            add_line!(self, "void *{external_scanner_name}_create(void);");
            add_line!(self, "void {external_scanner_name}_destroy(void *);");
            add_line!(
                self,
                "bool {external_scanner_name}_scan(void *, TSLexer *, const bool *);",
            );
            add_line!(
                self,
                "unsigned {external_scanner_name}_serialize(void *, char *);",
            );
            add_line!(
                self,
                "void {external_scanner_name}_deserialize(void *, const char *, unsigned);",
            );
            add_line!(self, "");
        }

        if self.external_state_stride > 0 {
            add_line!(
                self,
                "static bool ts_external_scanner_scan(void *payload, TSLexer *lexer, const bool *states) {{"
            );
            indent!(self);
            add_line!(self, "const uint8_t *bits = (const uint8_t *)states;");
            add_line!(self, "bool valid[EXTERNAL_TOKEN_COUNT];");
            add_line!(
                self,
                "for (unsigned i = 0; i < EXTERNAL_TOKEN_COUNT; i++) valid[i] = (bits[i / 8] >> (i % 8)) & 1;"
            );
            add_line!(
                self,
                "return {external_scanner_name}_scan(payload, lexer, valid);"
            );
            dedent!(self);
            add_line!(self, "}}");
        }
        add_line!(self, "#ifdef TREE_SITTER_HIDE_SYMBOLS");
        add_line!(self, "#define TS_PUBLIC");
        add_line!(self, "#elif defined(_WIN32)");
        add_line!(self, "#define TS_PUBLIC __declspec(dllexport)");
        add_line!(self, "#else");
        add_line!(
            self,
            "#define TS_PUBLIC __attribute__((visibility(\"default\")))"
        );
        add_line!(self, "#endif");
        add_line!(self, "");

        add_line!(
            self,
            "TS_PUBLIC const TSLanguage *{language_function_name}(void) {{",
        );
        indent!(self);
        add_line!(self, "static const TSLanguage language = {{");
        indent!(self);
        add_line!(self, ".abi_version = LANGUAGE_VERSION,");

        // Quantities
        add_line!(self, ".symbol_count = SYMBOL_COUNT,");
        add_line!(self, ".alias_count = ALIAS_COUNT,");
        add_line!(self, ".token_count = TOKEN_COUNT,");
        add_line!(self, ".external_token_count = EXTERNAL_TOKEN_COUNT,");
        add_line!(self, ".state_count = STATE_COUNT,");
        add_line!(self, ".large_state_count = LARGE_STATE_COUNT,");
        add_line!(self, ".production_id_count = PRODUCTION_ID_COUNT,");
        if self.abi_version >= ABI_VERSION_WITH_RESERVED_WORDS {
            add_line!(self, ".supertype_count = SUPERTYPE_COUNT,");
        }
        add_line!(self, ".field_count = FIELD_COUNT,");
        add_line!(
            self,
            ".max_alias_sequence_length = MAX_ALIAS_SEQUENCE_LENGTH,"
        );

        // Parse table
        add_line!(self, ".parse_table = &ts_parse_table[0][0],");
        if self.large_state_count < self.parse_table.states.len() {
            add_line!(self, ".small_parse_table = ts_small_parse_table,");
            add_line!(self, ".small_parse_table_map = ts_small_parse_table_map,");
        }
        add_line!(self, ".parse_actions = ts_parse_actions,");

        // Metadata
        add_line!(self, ".symbol_names = ts_symbol_names,");
        if !self.field_names.is_empty() {
            add_line!(self, ".field_names = ts_field_names,");
            add_line!(self, ".field_map_slices = ts_field_map_slices,");
            add_line!(self, ".field_map_entries = ts_field_map_entries,");
        }
        if !self.supertype_map.is_empty() && self.abi_version >= ABI_VERSION_WITH_RESERVED_WORDS {
            add_line!(self, ".supertype_map_slices = ts_supertype_map_slices,");
            add_line!(self, ".supertype_map_entries = ts_supertype_map_entries,");
            add_line!(self, ".supertype_symbols = ts_supertype_symbols,");
        }
        add_line!(self, ".symbol_metadata = ts_symbol_metadata,");
        add_line!(self, ".public_symbol_map = ts_symbol_map,");
        add_line!(self, ".alias_map = ts_non_terminal_alias_map,");
        if !self.parse_table.production_infos.is_empty() {
            if self.abi_version >= ABI_VERSION_WITH_COMPACT_TABLES {
                add_line!(self, ".alias_sequences = ts_alias_sequences,");
                add_line!(self, ".alias_sequence_offsets = ts_alias_sequence_offsets,");
                add_line!(
                    self,
                    ".alias_sequence_count = {},",
                    self.alias_sequence_count
                );
            } else {
                add_line!(self, ".alias_sequences = &ts_alias_sequences[0][0],");
            }
        }

        // Lexing
        add_line!(self, ".lex_modes = (const void*)ts_lex_modes,");
        if self.lex_mode_count > 0 {
            add_line!(self, ".lex_mode_ids = ts_lex_mode_ids,");
            add_line!(self, ".lex_mode_count = {},", self.lex_mode_count);
        }
        add_line!(self, ".lex_fn = ts_lex,");
        if let Some(keyword_capture_token) = self.syntax_grammar.word_token {
            add_line!(self, ".keyword_lex_fn = ts_lex_keywords,");
            if self.uses_keyword_table {
                add_line!(self, ".keyword_lookup_fn = ts_keyword_lookup,");
            }
            add_line!(
                self,
                ".keyword_capture_token = {},",
                self.symbol_ids[&keyword_capture_token]
            );
        }

        if !self.syntax_grammar.external_tokens.is_empty() {
            add_line!(self, ".external_scanner = {{");
            indent!(self);
            add_line!(self, "(const bool *)&ts_external_scanner_states[0][0],");
            add_line!(self, "ts_external_scanner_symbol_map,");
            add_line!(self, "{external_scanner_name}_create,");
            add_line!(self, "{external_scanner_name}_destroy,");
            if self.external_state_stride > 0 {
                add_line!(self, "ts_external_scanner_scan,");
            } else {
                add_line!(self, "{external_scanner_name}_scan,");
            }
            add_line!(self, "{external_scanner_name}_serialize,");
            add_line!(self, "{external_scanner_name}_deserialize,");
            dedent!(self);
            add_line!(self, "}},");
        }

        if self.external_state_stride > 0 {
            add_line!(
                self,
                ".external_state_stride = {},",
                self.external_state_stride
            );
        }
        add_line!(self, ".primary_state_ids = ts_primary_state_ids,");

        if self.abi_version >= ABI_VERSION_WITH_RESERVED_WORDS {
            add_line!(self, ".name = \"{}\",", self.language_name);

            if self.reserved_word_sets.len() > 1 {
                if self.uses_reserved_word_slices {
                    add_line!(self, ".reserved_words = ts_reserved_words,");
                    add_line!(self, ".reserved_word_slices = ts_reserved_word_slices,");
                } else {
                    add_line!(self, ".reserved_words = &ts_reserved_words[0][0],");
                }
            }

            add_line!(
                self,
                ".max_reserved_word_set_size = {},",
                self.reserved_word_sets
                    .iter()
                    .map(TokenSet::len)
                    .max()
                    .unwrap()
            );

            let metadata = self.metadata.unwrap_or_default();

            add_line!(self, ".metadata = {{");
            indent!(self);
            add_line!(self, ".major_version = {},", metadata.major);
            add_line!(self, ".minor_version = {},", metadata.minor);
            add_line!(self, ".patch_version = {},", metadata.patch);
            dedent!(self);
            add_line!(self, "}},");
        }

        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "return &language;");
        dedent!(self);
        add_line!(self, "}}");
        add_line!(self, "#ifdef __cplusplus");
        add_line!(self, "}}");
        add_line!(self, "#endif");
    }

    fn get_parse_action_list_id(
        id: ActionListId,
        pool: &ActionListPool,
        parse_action_list_offsets: &mut FxHashMap<ActionListId, u32>,
        next_parse_action_list_index: &mut u32,
    ) -> u32 {
        if let Some(&index) = parse_action_list_offsets.get(&id) {
            index
        } else {
            let result = *next_parse_action_list_index;
            parse_action_list_offsets.insert(id, result);
            *next_parse_action_list_index += 1 + pool.get(id).len() as u32;
            result
        }
    }

    fn external_token_id(&self, token_idx: usize) -> String {
        let token = &self.syntax_grammar.external_tokens[token_idx];
        format!("ts_external_token_{}", self.sanitize_identifier(token.name))
    }

    fn assign_symbol_id(
        &mut self,
        symbol: Symbol,
        used_identifiers: &mut FxHashSet<String>,
        suffixes: &mut FxHashMap<String, usize>,
    ) {
        let mut id;
        if symbol == Symbol::end() {
            id = "ts_builtin_sym_end".to_string();
        } else {
            let (name, kind) = self.metadata_for_symbol(symbol);
            id = match kind {
                VariableType::Auxiliary => format!("aux_sym_{}", self.sanitize_identifier(name)),
                VariableType::Anonymous => format!("anon_sym_{}", self.sanitize_identifier(name)),
                VariableType::Hidden | VariableType::Named => {
                    format!("sym_{}", self.sanitize_identifier(name))
                }
            };

            if used_identifiers.contains(&id) {
                let base_len = id.len();
                let suffix = suffixes.entry(id.clone()).or_insert(1);
                loop {
                    *suffix += 1;
                    id.truncate(base_len);
                    write!(&mut id, "{suffix}").unwrap();
                    if !used_identifiers.contains(&id) {
                        break;
                    }
                }
            }
        }

        used_identifiers.insert(id.clone());
        self.symbol_ids.insert(symbol, id);
    }

    fn field_id(field_name: &str) -> String {
        format!("field_{field_name}")
    }

    fn metadata_for_symbol(&self, symbol: Symbol) -> (StrId, VariableType) {
        let symbol_index = symbol.index as usize;
        match symbol.kind {
            SymbolType::End | SymbolType::EndOfNonTerminalExtra => {
                (StrPool::END_NAME_ID, VariableType::Hidden)
            }
            SymbolType::NonTerminal => {
                let variable = &self.syntax_grammar.variables[symbol_index];
                (variable.name, variable.kind)
            }
            SymbolType::Terminal => {
                let variable = &self.lexical_grammar.variables[symbol_index];
                (variable.name, variable.kind)
            }
            SymbolType::External => {
                let token = &self.syntax_grammar.external_tokens[symbol_index];
                (token.name, token.kind)
            }
        }
    }

    fn symbols_for_alias(&self, alias: Alias) -> &[Symbol] {
        self.symbols_by_node_identity
            .get(&(alias.value, alias.kind()))
            .map_or(&[], Vec::as_slice)
    }

    fn sanitize_identifier(&self, name: StrId) -> String {
        let name = self.str_pool.resolve(name);
        let mut result = String::with_capacity(name.len());
        for c in name.chars() {
            if c.is_ascii_alphanumeric() || c == '_' {
                result.push(c);
            } else {
                'special_chars: {
                    let replacement = match c {
                        ' ' if name.len() == 1 => "SPACE",
                        '~' => "TILDE",
                        '`' => "BQUOTE",
                        '!' => "BANG",
                        '@' => "AT",
                        '#' => "POUND",
                        '$' => "DOLLAR",
                        '%' => "PERCENT",
                        '^' => "CARET",
                        '&' => "AMP",
                        '*' => "STAR",
                        '(' => "LPAREN",
                        ')' => "RPAREN",
                        '-' => "DASH",
                        '+' => "PLUS",
                        '=' => "EQ",
                        '{' => "LBRACE",
                        '}' => "RBRACE",
                        '[' => "LBRACK",
                        ']' => "RBRACK",
                        '\\' => "BSLASH",
                        '|' => "PIPE",
                        ':' => "COLON",
                        ';' => "SEMI",
                        '"' => "DQUOTE",
                        '\'' => "SQUOTE",
                        '<' => "LT",
                        '>' => "GT",
                        ',' => "COMMA",
                        '.' => "DOT",
                        '?' => "QMARK",
                        '/' => "SLASH",
                        '\n' => "LF",
                        '\r' => "CR",
                        '\t' => "TAB",
                        '\0' => "NULL",
                        '\u{0001}' => "SOH",
                        '\u{0002}' => "STX",
                        '\u{0003}' => "ETX",
                        '\u{0004}' => "EOT",
                        '\u{0005}' => "ENQ",
                        '\u{0006}' => "ACK",
                        '\u{0007}' => "BEL",
                        '\u{0008}' => "BS",
                        '\u{000b}' => "VTAB",
                        '\u{000c}' => "FF",
                        '\u{000e}' => "SO",
                        '\u{000f}' => "SI",
                        '\u{0010}' => "DLE",
                        '\u{0011}' => "DC1",
                        '\u{0012}' => "DC2",
                        '\u{0013}' => "DC3",
                        '\u{0014}' => "DC4",
                        '\u{0015}' => "NAK",
                        '\u{0016}' => "SYN",
                        '\u{0017}' => "ETB",
                        '\u{0018}' => "CAN",
                        '\u{0019}' => "EM",
                        '\u{001a}' => "SUB",
                        '\u{001b}' => "ESC",
                        '\u{001c}' => "FS",
                        '\u{001d}' => "GS",
                        '\u{001e}' => "RS",
                        '\u{001f}' => "US",
                        '\u{007F}' => "DEL",
                        '\u{FEFF}' => "BOM",
                        '\u{0080}'..='\u{FFFF}' => {
                            write!(result, "u{:04x}", c as u32).unwrap();
                            break 'special_chars;
                        }
                        '\u{10000}'..='\u{10FFFF}' => {
                            write!(result, "U{:08x}", c as u32).unwrap();
                            break 'special_chars;
                        }
                        '0'..='9' | 'a'..='z' | 'A'..='Z' | '_' => unreachable!(),
                        ' ' => break 'special_chars,
                    };
                    if !result.is_empty() && !result.ends_with('_') {
                        result.push('_');
                    }
                    result += replacement;
                }
            }
        }
        result
    }

    fn sanitize_string(&self, name: StrId) -> String {
        let name = self.str_pool.resolve(name);
        let mut result = String::with_capacity(name.len());
        for c in name.chars() {
            match c {
                '\"' => result += "\\\"",
                '?' => result += "\\?",
                '\\' => result += "\\\\",
                '\u{0007}' => result += "\\a",
                '\u{0008}' => result += "\\b",
                '\u{000b}' => result += "\\v",
                '\u{000c}' => result += "\\f",
                '\n' => result += "\\n",
                '\r' => result += "\\r",
                '\t' => result += "\\t",
                '\0' => result += "\\0",
                '\u{0001}'..='\u{001f}' => write!(result, "\\x{:02x}", c as u32).unwrap(),
                '\u{007F}'..='\u{FFFF}' => write!(result, "\\u{:04x}", c as u32).unwrap(),
                '\u{10000}'..='\u{10FFFF}' => write!(result, "\\U{:08x}", c as u32).unwrap(),
                _ => result.push(c),
            }
        }
        result
    }

    fn add_character(&mut self, c: char) {
        match c {
            '\'' => add!(self, "'\\''"),
            '\\' => add!(self, "'\\\\'"),
            '\u{000c}' => add!(self, "'\\f'"),
            '\n' => add!(self, "'\\n'"),
            '\t' => add!(self, "'\\t'"),
            '\r' => add!(self, "'\\r'"),
            _ => {
                if c == '\0' {
                    add!(self, "0");
                } else if c == ' ' || c.is_ascii_graphic() {
                    add!(self, "'{c}'");
                } else {
                    add!(self, "0x{:02x}", c as u32);
                }
            }
        }
    }
}

/// Returns a String of C code for the given components of a parser.
///
/// # Arguments
///
/// * `name` - A string slice containing the name of the language
/// * `parse_table` - The generated parse table for the language
/// * `main_lex_table` - The generated lexing table for the language
/// * `keyword_lex_table` - The generated keyword lexing table for the language
/// * `keyword_capture_token` - A symbol indicating which token is used for keyword capture, if any.
/// * `syntax_grammar` - The syntax grammar extracted from the language's grammar
/// * `lexical_grammar` - The lexical grammar extracted from the language's grammar
/// * `default_aliases` - A map describing the global rename rules that should apply. the keys are
///   symbols that are *always* aliased in the same way, and the values are the aliases that are
///   applied to those symbols.
/// * `str_pool` - A backing pool for `StrId`-identified strings within `syntax_grammar`,
///   `lexical_grammar`, and `default_aliases`.
/// * `abi_version` - The language ABI version that should be generated. Usually you want
///   Tree-sitter's current version, but right after making an ABI change, it may be useful to
///   generate code with the previous ABI.
#[expect(
    clippy::too_many_arguments,
    reason = "all parameters are required for code generation"
)]
pub fn render_c_code(
    name: StrId,
    tables: Tables,
    syntax_grammar: SyntaxGrammar,
    lexical_grammar: LexicalGrammar,
    default_aliases: AliasMap,
    str_pool: StrPool,
    abi_version: usize,
    semantic_version: Option<(u8, u8, u8)>,
    supertype_symbol_map: BTreeMap<Symbol, Vec<ChildType>>,
    profile: Option<&GenerationProfile>,
) -> RenderResult<String> {
    if !(ABI_VERSION_MIN..=ABI_VERSION_MAX).contains(&abi_version) {
        Err(RenderError::ABI(abi_version))?;
    }

    Generator {
        language_name: str_pool.resolve(name).to_string(),
        profile: profile.cloned(),
        parse_table: tables.parse_table,
        main_lex_table: tables.main_lex_table,
        keyword_lex_table: tables.keyword_lex_table,
        large_character_sets: tables.large_character_sets,
        large_character_set_info: Vec::new(),
        syntax_grammar,
        lexical_grammar,
        default_aliases,
        abi_version,
        metadata: semantic_version.map(|(major, minor, patch)| Metadata {
            major,
            minor,
            patch,
        }),
        supertype_symbol_map,
        str_pool,
        ..Default::default()
    }
    .generate()
}
