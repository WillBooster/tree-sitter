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
    nfa::{CharacterSet, NfaCursor, NfaState},
    node_types::ChildType,
    rules::Alias,
    rules::{AliasMap, Symbol, SymbolType, TokenSet},
    strpool::{StrId, StrPool},
    tables::{AdvanceAction, GotoAction, LexState, LexTable, ParseAction, ParseTable},
};

const BITMAP_STATE_MIN_ENTRIES: usize = 16;
/// The largest count that fits in the 7 bits of a packed action list header.
const MAX_ACTION_LIST_LENGTH: usize = 127;
const MAX_GROUPED_LOOKUP_STEPS: usize = 16;
/// Marks a goto map entry that refers to a descriptor of two 32-bit words: the offset of the state's set of
/// nonterminals with gotos, and the map entry (offset and encoding flags) of the row of its gotos that differ
/// from their nonterminal's default.
const SMALL_STATE_DEFAULTS_FLAG: usize = 0x2000_0000;
const SMALL_STATE_BITMAP_FLAG: usize = 0x4000_0000;
const SMALL_STATE_PAIR_FLAG: usize = 0x8000_0000;
const SMALL_STATE_SET_FLAG: usize = SMALL_STATE_BITMAP_FLAG | SMALL_STATE_PAIR_FLAG;
const MAX_SINGLE_LEXER_STATES: usize = 4096;
const MAX_SINGLE_LEXER_BYTES: usize = 512 * 1024;
const LEXER_CHUNK_SIZE: usize = 256;
const KEYWORD_SKIP_FLAG: usize = 1 << 15;

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
    #[error("Sparse parse table offset {0} exceeds the address limit")]
    SparseTable(usize),
    #[error("Invalid generation profile: {0}")]
    Profile(String),
    #[error("Parse action table size of {0} words exceeds maximum value of {max}", max=u16::MAX)]
    ParseTable(usize),
    #[error("Parse action list length {0} exceeds maximum value of {max}", max=MAX_ACTION_LIST_LENGTH)]
    ActionListLength(usize),
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

struct SmallParseRow {
    index: usize,
    values: Vec<u16>,
}

type SmallStateGroups = Vec<((u32, SymbolType), Vec<Symbol>)>;

/// Word indices of reduce records, keyed by symbol, child count, dynamic precedence, and production id.
type ReduceRecords = IndexMap<(Symbol, u16, i32, u16), u32, FxBuildHasher>;

struct SharedParseRows {
    offset: usize,
    rows: Vec<SmallParseRow>,
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
    token_count: usize,
    /// The most common goto value of each nonterminal, by its position after the tokens.
    goto_defaults: Vec<u32>,
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
    external_token_ids: Vec<String>,
    alias_ids: FxHashMap<Alias, String>,
    unique_aliases: Vec<Alias>,
    symbol_map: FxHashMap<Symbol, Symbol>,
    symbols_by_node_identity: FxHashMap<(StrId, VariableType), Vec<Symbol>>,
    reserved_word_sets: Vec<TokenSet>,
    reserved_word_set_ids_by_parse_state: Vec<usize>,
    field_names: Vec<StrId>,
    supertype_symbol_map: BTreeMap<Symbol, Vec<ChildType>>,
    supertype_map: BTreeMap<String, Vec<ChildType>>,
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

        if !self.supertype_map.is_empty() {
            self.add_supertype_map();
        }

        let buffer_offset_before_lex_functions = self.buffer.len();

        let mut main_lex_table = LexTable::default();
        swap(&mut main_lex_table, &mut self.main_lex_table);
        self.add_lex_function("ts_lex", &main_lex_table);
        drop(main_lex_table);

        if self.syntax_grammar.word_token.is_some() {
            let mut keyword_lex_table = LexTable::default();
            swap(&mut keyword_lex_table, &mut self.keyword_lex_table);
            if !self.keyword_separators_overlap_word(&keyword_lex_table)
                && keyword_lex_table.states.len() >= 64
                && keyword_lex_table.states.len() < KEYWORD_SKIP_FLAG
                && keyword_lex_table.states.iter().all(|state| {
                    state.eof_action.is_none()
                        && state.advance_actions.iter().all(|(set, action)| {
                            set.ranges().all(|range| {
                                *range.start() > '\0'
                                    && (!action.in_main_token || range.end().is_ascii())
                            })
                        })
                })
            {
                self.uses_keyword_table = true;
                self.add_keyword_table(&keyword_lex_table);
            } else {
                self.add_lex_function("ts_lex_keywords", &keyword_lex_table);
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

        if self.reserved_word_sets.len() > 1 {
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
        if profile.parse_states.len() != n
            || profile.lex_states.len() != self.main_lex_table.states.len()
            || profile
                .edges
                .iter()
                .any(|&(a, b, _)| a as usize >= n || b as usize >= n)
            || !(2..=n).contains(&profile.max_dense_states)
        {
            return Err(RenderError::Profile("requires matching state counts, valid edges, and a dense-state limit between 2 and STATE_COUNT".into()));
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
                state.lex_state_id_after_extra =
                    lex_inverse[state.lex_state_id_after_extra as usize];
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
            let mut ranges = Vec::new();
            for (set, action) in &state.advance_actions {
                let destination = action.state
                    | if action.in_main_token {
                        0
                    } else {
                        KEYWORD_SKIP_FLAG as u32
                    };
                for range in set.ranges() {
                    let first = *range.start() as u32;
                    let last = *range.end() as u32;
                    if first < 128 {
                        ranges.push((first as u8, last.min(127) as u8, destination));
                    }
                }
            }
            ranges.sort_unstable();
            rows.push((index, state.accept_action));
            transitions.extend(ranges);
        }
        let transition_count = transitions.len();
        let mut first_byte = [u16::MAX; 256];
        for &(first, last, state) in &transitions[rows[0].0..rows[1].0] {
            first_byte[usize::from(first)..=usize::from(last)].fill(state as u16);
        }
        for (first, last, state) in transitions {
            add_line!(self, "{{ {first}, {last}, {state} }},");
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(
            self,
            "static const uint16_t ts_keyword_first_byte[256] = {{"
        );
        indent!(self);
        for values in first_byte.chunks(16) {
            add_line!(
                self,
                "{},",
                values
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
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

    fn keyword_separators_overlap_word(&self, table: &LexTable) -> bool {
        let has_unicode_separator = table.states.iter().any(|state| {
            state.advance_actions.iter().any(|(set, action)| {
                !action.in_main_token && set.ranges().any(|range| !range.end().is_ascii())
            })
        });
        if !has_unicode_separator {
            return false;
        }
        let word = self.syntax_grammar.word_token.unwrap();
        let nfa = &self.lexical_grammar.nfa;
        let cursor = NfaCursor::new(
            nfa,
            vec![self.lexical_grammar.variables[word.index as usize].start_state],
        );
        cursor.state_ids.iter().any(|&token_state| {
            let NfaState::Advance {
                chars,
                is_sep: false,
                ..
            } = &nfa.states[token_state as usize]
            else {
                return false;
            };
            table.states.iter().any(|state| {
                state.advance_actions.iter().any(|(separators, action)| {
                    !action.in_main_token && chars.does_intersect(separators)
                })
            })
        })
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
        self.external_token_ids = self
            .syntax_grammar
            .external_tokens
            .iter()
            .map(|token| {
                Self::unique_identifier(
                    format!("ts_external_token_{}", self.sanitize_identifier(token.name)),
                    &mut symbol_identifiers,
                    &mut symbol_identifier_suffixes,
                )
            })
            .collect();
        self.symbol_ids.insert(
            Symbol::end_of_nonterminal_extra(),
            self.symbol_ids[&Symbol::end()].clone(),
        );

        self.init_symbol_map();

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

        for (supertype, subtypes) in &self.supertype_symbol_map {
            if let Some(supertype) = self.symbol_ids.get(supertype) {
                self.supertype_map
                    .entry(supertype.clone())
                    .or_insert_with(|| subtypes.clone());
            }
        }

        self.supertype_symbol_map.clear();

        self.symbol_order.insert(Symbol::end(), 0);
        self.symbol_order
            .insert(Symbol::end_of_nonterminal_extra(), 0);
        for (i, symbol) in self
            .parse_table
            .symbols
            .iter()
            .filter(|symbol| **symbol != Symbol::end())
            .enumerate()
        {
            self.symbol_order.insert(*symbol, i + 1);
        }
        self.token_count = self.count_tokens();
        self.goto_defaults = self.compute_goto_defaults();
        self.large_state_count = self.count_large_states();
    }

    fn count_tokens(&self) -> usize {
        self.parse_table
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
            .count()
    }

    fn init_symbol_map(&mut self) {
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
    }

    fn compute_goto_defaults(&self) -> Vec<u32> {
        let mut counts = vec![
            FxHashMap::<u32, usize>::default();
            self.parse_table.symbols.len() - self.token_count
        ];
        for (state_id, state) in self.parse_table.states.iter().enumerate() {
            for (symbol, &action) in state.nonterminal_entries.iter() {
                let column = self.symbol_order[symbol] - self.token_count;
                *counts[column]
                    .entry(self.small_goto_value(action, state_id))
                    .or_default() += 1;
            }
        }
        counts
            .into_iter()
            .map(|column| {
                column
                    .into_iter()
                    .max_by_key(|&(value, count)| (count, std::cmp::Reverse(value)))
                    .map_or(0, |(value, _)| value)
            })
            .collect()
    }

    fn is_goto_default(&self, symbol: Symbol, value: u32) -> bool {
        self.goto_defaults[self.symbol_order[&symbol] - self.token_count] == value
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
        let minimum = self.parse_table.states.len().min(2);
        let dense_bytes = self.parse_table.symbols.len() * size_of::<u16>();
        let mut groups = FxHashMap::default();
        let mut row_ids = FxHashMap::default();
        let mut rows = Vec::new();
        let mut row_counts = Vec::new();
        let mut row_bytes = Vec::new();
        for (i, state) in self.parse_table.states.iter().enumerate().skip(minimum) {
            let terminal_row = state
                .terminal_entries
                .iter()
                .map(|(&symbol, id)| (symbol, (id.index() as u32) * 2 + u32::from(id.reusable())));
            let goto_row = state
                .nonterminal_entries
                .iter()
                .map(|(&symbol, action)| (symbol, self.small_goto_value(*action, i)))
                .filter(|&(symbol, value)| !self.is_goto_default(symbol, value));
            let state_rows: [(bool, Vec<_>); 2] =
                [(false, terminal_row.collect()), (true, goto_row.collect())];
            for (is_goto, mut row) in state_rows {
                row.sort_unstable_by_key(|&(symbol, _)| symbol);
                let next_id = row_counts.len();
                let id = *row_ids
                    .entry((is_goto, row))
                    .or_insert_with_key(|(_, row)| {
                        row_counts.push(0usize);
                        groups.clear();
                        for &(_, value) in row {
                            *groups.entry(value).or_insert(0usize) += 1;
                        }
                        let (_, words) =
                            self.small_state_encoding(groups.values().copied(), false, is_goto);
                        row_bytes.push(words * size_of::<u16>());
                        next_id
                    });
                row_counts[id] += 1;
                rows.push(id);
            }
        }
        // Each small state has two map entries; goto descriptors and sets are not counted.
        let map_bytes = 2 * size_of::<u32>();
        let mut bytes =
            minimum * dense_bytes + rows.len() / 2 * map_bytes + row_bytes.iter().sum::<usize>();
        let mut best_bytes = bytes;
        let mut count = minimum;
        for (i, ids) in rows.chunks(2).enumerate() {
            bytes += dense_bytes;
            bytes -= map_bytes;
            for &id in ids {
                row_counts[id] -= 1;
                if row_counts[id] == 0 {
                    bytes -= row_bytes[id];
                }
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
        let token_count = self.token_count;

        add_line!(self, "#define LANGUAGE_VERSION {LANGUAGE_VERSION:#x}");
        add_line!(
            self,
            "#define STATE_COUNT {}",
            self.parse_table.states.len()
        );
        add_line!(self, "#define LARGE_STATE_COUNT {}", self.large_state_count);
        add_line!(
            self,
            "#define LEX_STATE_COUNT {}",
            self.main_lex_table.states.len()
        );

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
        let mut i = 1;
        for symbol in &self.parse_table.symbols {
            if *symbol != Symbol::end() {
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
            if symbol.is_terminal()
                && self.lexical_grammar.variables[symbol.index as usize].is_immediate
            {
                add_line!(self, ".immediate = true,");
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

    fn add_lex_function(&mut self, name: &str, lex_table: &LexTable) {
        if lex_table
            .states
            .iter()
            .all(|state| state.accept_action.is_none() && state.fallback_accept.is_none())
        {
            add_line!(
                self,
                "static bool {name}(TSLexer *lexer, TSStateId state) {{ (void)lexer; (void)state; return false; }}"
            );
            add_line!(self, "");
            return;
        }
        let has_fallback = lex_table
            .states
            .iter()
            .any(|state| state.fallback_accept.is_some() || state.restore_fallback);
        if has_fallback {
            add_line!(
                self,
                "typedef struct {{ bool valid; int32_t precedence; TSSymbol symbol; TSLexerPosition start, end; }} {name}_fallback;"
            );
        }
        if lex_table.states.len() > MAX_SINGLE_LEXER_STATES {
            self.add_chunked_lex_function(name, lex_table, has_fallback);
            return;
        }
        let function_start = self.buffer.len();
        self.add_single_lex_function(name, lex_table, has_fallback);
        if self.buffer.len() - function_start > MAX_SINGLE_LEXER_BYTES {
            self.buffer.truncate(function_start);
            self.add_chunked_lex_function(name, lex_table, has_fallback);
        }
    }

    fn add_single_lex_function(&mut self, name: &str, lex_table: &LexTable, has_fallback: bool) {
        add_line!(
            self,
            "static bool {name}(TSLexer *lexer, TSStateId state) {{",
        );
        indent!(self);

        let tag_count = Self::start_tag_count(lex_table);
        add_line!(self, "UNUSED TSLexerPosition start_tags[{tag_count}];");
        if has_fallback {
            add_line!(
                self,
                "{name}_fallback fallback_storage = {{0}}, *fallback = &fallback_storage;"
            );
        }
        add_line!(self, "START_LEXER();");
        add_line!(self, "eof = lookahead == 0 && lexer->eof(lexer);");
        add_line!(self, "switch (state) {{");

        indent!(self);
        for (i, state) in lex_table.states.iter().enumerate() {
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

    fn start_tag_count(table: &LexTable) -> u32 {
        table
            .states
            .iter()
            .flat_map(|state| {
                state
                    .start_tag
                    .into_iter()
                    .chain(state.fallback_accept.and_then(|(_, _, tag)| tag))
            })
            .chain(table.states.iter().flat_map(|state| {
                state
                    .advance_actions
                    .iter()
                    .map(|(_, action)| action)
                    .chain(state.eof_action.iter())
                    .flat_map(|action| action.start_tags.iter().map(|&(tag, _)| tag))
            }))
            .max()
            .map_or(1, |tag| tag + 1)
    }

    fn add_chunked_lex_function(&mut self, name: &str, lex_table: &LexTable, has_fallback: bool) {
        let chunk_count = lex_table.states.len().div_ceil(LEXER_CHUNK_SIZE);
        let fallback_parameter = if has_fallback {
            format!(", {name}_fallback *")
        } else {
            String::new()
        };
        let fallback_argument = if has_fallback { ", &fallback" } else { "" };
        for chunk in 0..chunk_count {
            add_line!(
                self,
                "static uint8_t {name}_{chunk}(TSLexer *, TSStateId *, bool *, TSLexerPosition *{fallback_parameter});"
            );
        }
        add_line!(self, "");
        add_line!(
            self,
            "static bool {name}(TSLexer *lexer, TSStateId state) {{"
        );
        indent!(self);
        let tag_count = Self::start_tag_count(lex_table);
        add_line!(self, "UNUSED TSLexerPosition start_tags[{tag_count}];");
        if has_fallback {
            add_line!(self, "{name}_fallback fallback = {{0}};");
        }
        add_line!(self, "bool result = false;");
        add_line!(self, "for (;;) {{");
        indent!(self);
        add_line!(self, "uint8_t status;");
        add_line!(self, "switch (state / {LEXER_CHUNK_SIZE}) {{");
        indent!(self);
        for chunk in 0..chunk_count {
            add_line!(
                self,
                "case {chunk}: status = {name}_{chunk}(lexer, &state, &result, start_tags{fallback_argument}); break;"
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
            self.add_lex_chunk(name, chunk, states, has_fallback);
        }
    }

    fn add_lex_chunk(&mut self, name: &str, chunk: usize, states: &[LexState], has_fallback: bool) {
        let fallback_parameter = if has_fallback {
            format!(", {name}_fallback *fallback")
        } else {
            String::new()
        };
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
            "static uint8_t {name}_{chunk}(TSLexer *lexer, TSStateId *state_out, bool *result_out, TSLexerPosition *start_tags{fallback_parameter}) {{"
        );
        indent!(self);
        add_line!(self, "(void)start_tags;");
        if has_fallback {
            add_line!(self, "(void)fallback;");
        }
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
            self.add_lex_state(id, state);
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

    fn add_lex_state(&mut self, state_id: u32, state: &LexState) {
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
        if !defer_accept {
            self.add_lex_accept(state);
        }

        if let Some(ref eof_action) = state.eof_action {
            add_whitespace!(self);
            add!(self, "if (eof) ");
            self.add_advance_action(eof_action, state_id);
            add!(self, "\n");
        }

        let mut chars_copy = CharacterSet::empty();
        let mut large_set = CharacterSet::empty();
        let mut ruled_out_chars = CharacterSet::empty();

        let mut mapped_transitions = vec![false; state.advance_actions.len()];
        let mut simple_transition_range_count = 0;
        for (index, (chars, action)) in state.advance_actions.iter().enumerate() {
            // Lookahead is also zero at EOF, so the map cannot distinguish a physical NUL.
            if action.in_main_token
                && action.start_tags.is_empty()
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
            // Rendered ranges start and end at Unicode scalar values, so a transition on
            // surrogate code points alone has no condition to render.
            if mapped_transitions[index] || chars.ranges().next().is_none() {
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

        if defer_accept {
            self.add_lex_accept(state);
        }
        add_line!(self, "END_STATE();");
    }

    fn add_lex_accept(&mut self, state: &LexState) {
        if let Some((symbol, precedence, start_tag)) = state.fallback_accept {
            add_line!(
                self,
                "if (!fallback->valid || {precedence} >= fallback->precedence) {{"
            );
            indent!(self);
            add_line!(self, "fallback->valid = true;");
            add_line!(self, "fallback->precedence = {precedence};");
            add_line!(self, "fallback->symbol = {};", self.symbol_ids[&symbol]);
            if let Some(tag) = start_tag {
                add_line!(self, "fallback->start = start_tags[{tag}];");
            } else {
                add_line!(self, "lexer->get_position(lexer, &fallback->start);");
            }
            add_line!(self, "lexer->mark_end(lexer);");
            add_line!(self, "lexer->get_mark_end(lexer, &fallback->end);");
            dedent!(self);
            add_line!(self, "}}");
        }
        if state.restore_fallback {
            add_line!(self, "result = fallback->valid;");
            add_line!(self, "if (result) {{");
            indent!(self);
            add_line!(self, "lexer->result_symbol = fallback->symbol;");
            add_line!(self, "lexer->mark_start(lexer, &fallback->start);");
            add_line!(self, "lexer->mark_end_at(lexer, &fallback->end);");
            dedent!(self);
            add_line!(self, "}}");
        } else if let Some(symbol) = state.accept_action {
            if let Some(precedence) = state.guarded_precedence {
                add_line!(
                    self,
                    "if (!fallback->valid || {precedence} >= fallback->precedence) {{"
                );
                indent!(self);
            }
            if let Some(tag) = state.start_tag {
                add_line!(self, "lexer->mark_start(lexer, &start_tags[{tag}]);");
            }
            add_line!(self, "ACCEPT_TOKEN({});", self.symbol_ids[&symbol]);
            if state.guarded_precedence.is_some() {
                dedent!(self);
                add_line!(self, "}}");
            }
        }
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
        if !action.start_tags.is_empty() {
            add!(self, "{{ ");
            for &(target, source) in &action.start_tags {
                if let Some(source) = source {
                    add!(self, "start_tags[{target}] = start_tags[{source}]; ");
                } else {
                    add!(self, "lexer->get_position(lexer, &start_tags[{target}]); ");
                }
            }
        }
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
        if !action.start_tags.is_empty() {
            add!(self, " }}");
        }
    }

    fn add_lex_modes(&mut self) {
        if self.add_shared_lex_modes() {
            return;
        }

        add_line!(
            self,
            "static const TSLexerMode ts_lex_modes[STATE_COUNT] = {{"
        );
        indent!(self);
        for (i, state) in self.parse_table.states.iter().enumerate() {
            add_whitespace!(self);
            add!(self, "[{i}] = {{");
            if state.is_end_of_non_terminal_extra() {
                add!(self, "(TSStateId)(-1),");
            } else {
                add!(
                    self,
                    ".lex_state = {}, .lex_state_after_extra = {}",
                    state.lex_state_id,
                    state.lex_state_id_after_extra
                );

                if state.external_lex_state_id > 0 {
                    add!(
                        self,
                        ", .external_lex_state = {}, .external_lex_state_after_extra = {}",
                        state.external_lex_state_id,
                        state.external_lex_state_id_after_extra
                    );
                }

                let reserved_word_set_id = self.reserved_word_set_ids_by_parse_state[i];
                if reserved_word_set_id != 0 {
                    add!(self, ", .reserved_word_set_id = {reserved_word_set_id}");
                }
            }

            add!(self, "}},\n");
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_shared_lex_modes(&mut self) -> bool {
        let mut modes = IndexMap::<(u32, u32, u32, u32, usize), usize, FxBuildHasher>::default();
        let mut ids = Vec::new();
        for (id, state) in self.parse_table.states.iter().enumerate() {
            let mode = if state.is_end_of_non_terminal_extra() {
                (u32::from(u16::MAX), u32::from(u16::MAX), 0, 0, 0)
            } else {
                (
                    state.lex_state_id,
                    state.lex_state_id_after_extra,
                    state.external_lex_state_id,
                    state.external_lex_state_id_after_extra,
                    self.reserved_word_set_ids_by_parse_state[id],
                )
            };
            let next = modes.len();
            ids.push(*modes.entry(mode).or_insert(next));
        }
        if modes.len() * 10 + ids.len() * 2 >= ids.len() * 10 {
            return false;
        }
        self.lex_mode_count = modes.len();
        add_line!(self, "static const TSLexerMode ts_lex_modes[] = {{");
        indent!(self);
        for (&(lex, after_extra, external, external_after_extra, reserved), &id) in &modes {
            add_line!(
                self,
                "[{id}] = {{.lex_state = {lex}, .lex_state_after_extra = {after_extra}, .external_lex_state = {external}, .external_lex_state_after_extra = {external_after_extra}, .reserved_word_set_id = {reserved}}},"
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
        if u16::try_from(total).is_ok()
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
        if self.profile.is_none()
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

        self.add_dense_parse_table(&mut parse_table_entries, &mut next_parse_action_list_index);
        self.add_small_parse_table(&mut parse_table_entries, &mut next_parse_action_list_index)?;

        let mut parse_table_entries = parse_table_entries
            .into_iter()
            .map(|(id, i)| (i, id))
            .collect::<Vec<_>>();
        parse_table_entries.sort_by_key(|(index, _)| *index);
        self.add_parse_action_list(parse_table_entries, next_parse_action_list_index)
    }

    fn add_dense_parse_table(
        &mut self,
        parse_table_entries: &mut FxHashMap<ActionListId, u32>,
        next_parse_action_list_index: &mut u32,
    ) {
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
                    parse_table_entries,
                    next_parse_action_list_index,
                );
                add_line!(self, "[{}] = ACTIONS({entry_id}),", self.symbol_ids[symbol]);
            }

            dedent!(self);
            add_line!(self, "}},");
        }

        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn add_small_parse_table(
        &mut self,
        parse_table_entries: &mut FxHashMap<ActionListId, u32>,
        next_parse_action_list_index: &mut u32,
    ) -> RenderResult<()> {
        if self.large_state_count == self.parse_table.states.len() {
            return Ok(());
        }
        add_line!(self, "static const uint16_t ts_small_parse_table[] = {{");
        indent!(self);
        // The table starts with each nonterminal's default goto, indexed by its position after the tokens.
        let defaults = SmallParseRow {
            index: 0,
            values: self
                .goto_defaults
                .iter()
                .map(|&value| value as u16)
                .collect(),
        };
        self.add_small_parse_row(&defaults);
        let mut next_index = defaults.values.len();
        let mut indices = Vec::new();
        let mut row_offsets = FxHashMap::default();
        let mut symbol_sets = FxHashMap::default();
        let mut goto_descriptors = FxHashMap::default();
        let has_samples = self
            .profile
            .as_ref()
            .is_some_and(|profile| profile.parse_states.iter().any(|&count| count > 0));
        for state_id in self.large_state_count..self.parse_table.states.len() {
            let minimize = has_samples
                && self
                    .profile
                    .as_ref()
                    .is_some_and(|profile| profile.parse_states[state_id] == 0);
            let (terminal_groups, goto_groups) = self.small_state_groups(
                state_id,
                parse_table_entries,
                next_parse_action_list_index,
            );
            // States that training visited keep complete goto rows, which take one lookup instead of three.
            let uses_goto_defaults = !has_samples || minimize;
            let goto_symbols = self.goto_symbols(state_id);
            let mut goto_groups = goto_groups
                .into_iter()
                .filter_map(|((value, kind), mut symbols)| {
                    symbols.retain(|&symbol| {
                        !uses_goto_defaults || !self.is_goto_default(symbol, value)
                    });
                    (!symbols.is_empty()).then_some(((value, kind), symbols))
                })
                .collect::<Vec<_>>();
            goto_groups
                .sort_unstable_by_key(|((value, _), symbols)| (symbols.len(), *value, symbols[0]));
            let mut offsets = [0; 2];
            for (is_goto, groups) in [(false, terminal_groups), (true, goto_groups)] {
                let key = (is_goto, self.encode_small_parse_row(&groups, 0, is_goto));
                offsets[usize::from(is_goto)] = if let Some(&offset) = row_offsets.get(&key) {
                    offset
                } else {
                    let offset = self.add_small_parse_rows(
                        &groups,
                        minimize,
                        is_goto,
                        &mut symbol_sets,
                        &mut next_index,
                    )?;
                    row_offsets.insert(key, offset);
                    offset
                };
            }
            indices.push(offsets[0]);
            if !uses_goto_defaults {
                indices.push(offsets[1]);
                continue;
            }
            let descriptor = self.add_goto_descriptor(
                &goto_symbols,
                offsets[1],
                &mut symbol_sets,
                &mut goto_descriptors,
                &mut next_index,
            )?;
            indices.push(descriptor | SMALL_STATE_DEFAULTS_FLAG);
        }
        dedent!(self);
        add_line!(self, "}};\n");
        add_line!(
            self,
            "static const uint32_t ts_small_parse_table_map[] = {{"
        );
        indent!(self);
        for (state, offsets) in (self.large_state_count..).zip(indices.chunks(2)) {
            add_line!(
                self,
                "[SMALL_STATE({state}) * 2] = {}, {},",
                offsets[0],
                offsets[1]
            );
        }
        dedent!(self);
        add_line!(self, "}};\n");
        Ok(())
    }

    /// Adds a descriptor of the set of `goto_symbols` and the goto row at `goto_row`, and returns its offset.
    fn add_goto_descriptor(
        &mut self,
        goto_symbols: &[Symbol],
        goto_row: usize,
        symbol_sets: &mut FxHashMap<Vec<u16>, usize>,
        descriptors: &mut FxHashMap<Vec<u16>, usize>,
        next_index: &mut usize,
    ) -> RenderResult<usize> {
        let set = if goto_symbols.is_empty() {
            vec![0, 0]
        } else {
            self.symbol_set(goto_symbols)
        };
        let set_offset = self.add_shared_values(set, symbol_sets, next_index)?;
        let descriptor = [set_offset, goto_row]
            .into_iter()
            .flat_map(|offset| [(offset & 0xffff) as u16, (offset >> 16) as u16])
            .collect();
        self.add_shared_values(descriptor, descriptors, next_index)
    }

    /// Adds `values` as a row unless an identical row was added to `rows` before, and returns its offset.
    fn add_shared_values(
        &mut self,
        values: Vec<u16>,
        rows: &mut FxHashMap<Vec<u16>, usize>,
        next_index: &mut usize,
    ) -> RenderResult<usize> {
        if let Some(&offset) = rows.get(&values) {
            return Ok(offset);
        }
        let row = SmallParseRow {
            index: *next_index,
            values,
        };
        self.add_small_parse_row(&row);
        *next_index += row.values.len();
        if *next_index >= SMALL_STATE_DEFAULTS_FLAG {
            return Err(RenderError::SparseTable(*next_index));
        }
        rows.insert(row.values, row.index);
        Ok(row.index)
    }

    /// Adds the rows encoding `groups` and returns the map entry that refers to them.
    fn add_small_parse_rows(
        &mut self,
        groups: &[((u32, SymbolType), Vec<Symbol>)],
        minimize: bool,
        is_goto: bool,
        symbol_sets: &mut FxHashMap<Vec<u16>, usize>,
        next_index: &mut usize,
    ) -> RenderResult<usize> {
        let (flag, words) = self.small_state_encoding(
            groups.iter().map(|(_, symbols)| symbols.len()),
            minimize,
            is_goto,
        );
        let shared = self.encode_shared_parse_row(
            groups,
            words,
            minimize || flag == 0,
            symbol_sets,
            next_index,
        )?;
        let offset = if let Some(shared) = shared {
            for row in shared.rows {
                self.add_small_parse_row(&row);
            }
            shared.offset
        } else {
            let row = SmallParseRow {
                index: *next_index,
                values: self.encode_small_parse_row(groups, flag, is_goto),
            };
            self.add_small_parse_row(&row);
            let offset = *next_index | flag;
            *next_index += row.values.len();
            offset
        };
        if *next_index >= SMALL_STATE_DEFAULTS_FLAG {
            return Err(RenderError::SparseTable(*next_index));
        }
        Ok(offset)
    }

    fn add_small_parse_row(&mut self, row: &SmallParseRow) {
        add_line!(self, "[{}] =", row.index);
        indent!(self);
        for chunk in row.values.chunks(16) {
            add_whitespace!(self);
            for value in chunk {
                add!(self, "{value}, ");
            }
            add!(self, "\n");
        }
        dedent!(self);
    }

    fn small_state_groups(
        &self,
        state_id: usize,
        parse_table_entries: &mut FxHashMap<ActionListId, u32>,
        next_parse_action_list_index: &mut u32,
    ) -> (SmallStateGroups, SmallStateGroups) {
        let state = &self.parse_table.states[state_id];
        let mut groups = FxHashMap::<(u32, SymbolType), Vec<Symbol>>::default();
        let mut terminals = state.terminal_entries.iter().collect::<Vec<_>>();
        terminals.sort_unstable_by_key(|(symbol, _)| self.symbol_order[symbol]);
        for (&symbol, &entry) in terminals {
            let value = Self::get_parse_action_list_id(
                entry,
                &self.parse_table.action_lists,
                parse_table_entries,
                next_parse_action_list_index,
            );
            groups
                .entry((value, SymbolType::Terminal))
                .or_default()
                .push(symbol);
        }
        for (&symbol, &action) in state.nonterminal_entries.iter() {
            let value = self.small_goto_value(action, state_id);
            groups
                .entry((value, SymbolType::NonTerminal))
                .or_default()
                .push(symbol);
        }
        let mut groups = groups.into_iter().collect::<Vec<_>>();
        groups.sort_unstable_by_key(|((value, kind), symbols)| {
            (symbols.len(), *kind, *value, symbols[0])
        });
        for (_, symbols) in &mut groups {
            symbols.sort_unstable_by_key(|symbol| self.symbol_order[symbol]);
        }
        groups
            .into_iter()
            .partition(|((_, kind), _)| *kind == SymbolType::Terminal)
    }

    /// The nonterminals that have gotos in a state, in symbol order.
    fn goto_symbols(&self, state_id: usize) -> Vec<Symbol> {
        let mut symbols = self.parse_table.states[state_id]
            .nonterminal_entries
            .keys()
            .copied()
            .collect::<Vec<_>>();
        symbols.sort_unstable_by_key(|symbol| self.symbol_order[symbol]);
        symbols
    }

    /// The symbols of a small state's terminal or goto row: the first symbol and the count.
    const fn small_row_symbols(&self, is_goto: bool) -> (usize, usize) {
        if is_goto {
            (
                self.token_count,
                self.parse_table.symbols.len() - self.token_count,
            )
        } else {
            (0, self.token_count)
        }
    }

    fn encode_small_parse_row(
        &self,
        groups: &[((u32, SymbolType), Vec<Symbol>)],
        flag: usize,
        is_goto: bool,
    ) -> Vec<u16> {
        if flag == 0 {
            let mut row = vec![groups.len() as u16];
            for ((value, _), symbols) in groups {
                row.extend([*value as u16, symbols.len() as u16]);
                row.extend(
                    symbols
                        .iter()
                        .map(|symbol| self.symbol_order[symbol] as u16),
                );
            }
            return row;
        }
        let mut entries = groups
            .iter()
            .flat_map(|((value, _), symbols)| {
                symbols
                    .iter()
                    .map(|symbol| (self.symbol_order[symbol], *value as u16))
            })
            .collect::<Vec<_>>();
        entries.sort_unstable_by_key(|&(symbol, _)| symbol);
        if flag == SMALL_STATE_PAIR_FLAG {
            let mut row = vec![entries.len() as u16];
            row.extend(
                entries
                    .into_iter()
                    .flat_map(|(symbol, value)| [symbol as u16, value]),
            );
            return row;
        }
        let (first_symbol, symbol_count) = self.small_row_symbols(is_goto);
        let blocks = symbol_count.div_ceil(16);
        let mut row = vec![0u16; blocks * 2];
        for &(symbol, _) in &entries {
            let symbol = symbol - first_symbol;
            row[(symbol / 16) * 2] |= 1 << (symbol % 16);
        }
        let mut rank = 0;
        for block in 0..blocks {
            row[block * 2 + 1] = rank;
            rank += row[block * 2].count_ones() as u16;
        }
        row.extend(entries.into_iter().map(|(_, value)| value));
        row
    }

    fn encode_shared_parse_row(
        &self,
        groups: &[((u32, SymbolType), Vec<Symbol>)],
        unshared_words: usize,
        allow_many_groups: bool,
        symbol_sets: &mut FxHashMap<Vec<u16>, usize>,
        next_table_index: &mut usize,
    ) -> RenderResult<Option<SharedParseRows>> {
        if groups.len() > 4 && !allow_many_groups {
            return Ok(None);
        }
        let sets = groups
            .iter()
            .map(|(_, symbols)| self.symbol_set(symbols))
            .collect::<Vec<_>>();
        let words = 1
            + groups.len() * 3
            + sets
                .iter()
                .filter(|set| !symbol_sets.contains_key(*set))
                .map(Vec::len)
                .sum::<usize>();
        if words >= unshared_words {
            return Ok(None);
        }
        if *next_table_index + words >= SMALL_STATE_DEFAULTS_FLAG {
            return Err(RenderError::SparseTable(*next_table_index + words));
        }
        let mut rows = Vec::new();
        let mut offsets = Vec::with_capacity(sets.len());
        for set in sets {
            let offset = *symbol_sets.entry(set.clone()).or_insert_with(|| {
                let offset = *next_table_index;
                rows.push(SmallParseRow {
                    index: offset,
                    values: set.clone(),
                });
                *next_table_index += set.len();
                offset
            });
            offsets.push(offset);
        }
        let offset = *next_table_index;
        let mut values = vec![groups.len() as u16];
        for (((value, _), _), set_offset) in groups.iter().zip(offsets) {
            values.extend([
                *value as u16,
                (set_offset & 0xffff) as u16,
                (set_offset >> 16) as u16,
            ]);
        }
        *next_table_index += values.len();
        rows.push(SmallParseRow {
            index: offset,
            values,
        });
        Ok(Some(SharedParseRows {
            offset: offset | SMALL_STATE_SET_FLAG,
            rows,
        }))
    }

    fn symbol_set(&self, symbols: &[Symbol]) -> Vec<u16> {
        let first = self.symbol_order[&symbols[0]] / 16;
        let last = self.symbol_order[symbols.last().unwrap()] / 16;
        let mut set = vec![0u16; 2 + last - first + 1];
        set[0] = first as u16;
        set[1] = (last - first + 1) as u16;
        for symbol in symbols {
            let id = self.symbol_order[symbol];
            set[2 + id / 16 - first] |= 1 << (id % 16);
        }
        set
    }

    fn small_state_encoding(
        &self,
        group_sizes: impl Iterator<Item = usize>,
        minimize: bool,
        is_goto: bool,
    ) -> (usize, usize) {
        let mut entry_count = 0;
        let mut group_count = 0;
        let mut lookup_steps = 0;
        for size in group_sizes {
            entry_count += size;
            group_count += 1;
            // Match ts_language_lookup_small_compact's linear/binary search split.
            lookup_steps += if size < 16 {
                size
            } else {
                size.ilog2() as usize + 1
            };
        }
        let grouped = (0, 1 + 2 * group_count + entry_count);
        let bitmap = (
            SMALL_STATE_BITMAP_FLAG,
            2 * self.small_row_symbols(is_goto).1.div_ceil(16) + entry_count,
        );
        let pairs = (SMALL_STATE_PAIR_FLAG, 1 + 2 * entry_count);
        if minimize {
            [bitmap, pairs, grouped]
                .into_iter()
                .min_by_key(|&(_, words)| words)
                .unwrap()
        } else if entry_count >= BITMAP_STATE_MIN_ENTRIES
            && (group_count >= 8
                || lookup_steps > MAX_GROUPED_LOOKUP_STEPS
                || bitmap.1 <= grouped.1)
        {
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
        if u16::try_from(self.parse_table.states.len()).is_ok() && destination == state_id as u32 {
            u32::from(u16::MAX)
        } else {
            destination
        }
    }

    fn add_parse_action_list(
        &mut self,
        parse_table_entries: Vec<(u32, ActionListId)>,
        reduce_records_start: u32,
    ) -> RenderResult<()> {
        let reduce_records = self.reduce_records(&parse_table_entries, reduce_records_start);
        let word_count = reduce_records_start as usize + 2 * reduce_records.len();
        if word_count > usize::from(u16::MAX) {
            Err(RenderError::ParseTable(word_count))?;
        }

        add_line!(self, "static const uint32_t ts_parse_actions[] = {{");
        indent!(self);
        for (i, id) in parse_table_entries {
            let actions = self.parse_table.action_lists.get(id);
            if actions.len() > MAX_ACTION_LIST_LENGTH {
                Err(RenderError::ActionListLength(actions.len()))?;
            }
            let actions = actions
                .iter()
                .map(|action| Self::packed_parse_action(action, &reduce_records))
                .collect::<Vec<_>>();
            add_line!(
                self,
                "[{i}] = ACTION_LIST({}, {}){}{},",
                actions.len(),
                id.reusable(),
                if actions.is_empty() { "" } else { " | " },
                actions.join(", ")
            );
        }
        for ((symbol, child_count, dynamic_precedence, production_id), index) in reduce_records {
            add_line!(
                self,
                "[{index}] = REDUCE_RECORD({}, {child_count}, {dynamic_precedence}, {production_id}),",
                self.symbol_ids[&symbol]
            );
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
        Ok(())
    }

    /// Assigns each distinct reduction a two-word record after the action lists.
    fn reduce_records(
        &self,
        parse_table_entries: &[(u32, ActionListId)],
        start: u32,
    ) -> ReduceRecords {
        let mut records = ReduceRecords::default();
        for &(_, id) in parse_table_entries {
            for action in self.parse_table.action_lists.get(id) {
                if let ParseAction::Reduce {
                    symbol,
                    child_count,
                    dynamic_precedence,
                    production_id,
                    ..
                } = *action
                {
                    let next = start + 2 * records.len() as u32;
                    records
                        .entry((symbol, child_count, dynamic_precedence, production_id))
                        .or_insert(next);
                }
            }
        }
        records
    }

    fn packed_parse_action(action: &ParseAction, reduce_records: &ReduceRecords) -> String {
        match *action {
            ParseAction::Accept => "ACCEPT_INPUT()".to_string(),
            ParseAction::Recover => "RECOVER()".to_string(),
            ParseAction::ShiftExtra => "SHIFT_EXTRA()".to_string(),
            ParseAction::Shift {
                state,
                is_repetition: true,
            } => format!("SHIFT_REPEAT({state})"),
            ParseAction::Shift { state, .. } => format!("SHIFT({state})"),
            ParseAction::Reduce {
                symbol,
                child_count,
                dynamic_precedence,
                production_id,
                is_extra,
            } => {
                let action = if is_extra { "REDUCE_EXTRA" } else { "REDUCE" };
                let record =
                    reduce_records[&(symbol, child_count, dynamic_precedence, production_id)];
                format!("{action}({record})")
            }
        }
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
        add_line!(self, ".supertype_count = SUPERTYPE_COUNT,");
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
        if !self.supertype_map.is_empty() {
            add_line!(self, ".supertype_map_slices = ts_supertype_map_slices,");
            add_line!(self, ".supertype_map_entries = ts_supertype_map_entries,");
            add_line!(self, ".supertype_symbols = ts_supertype_symbols,");
        }
        add_line!(self, ".symbol_metadata = ts_symbol_metadata,");
        add_line!(self, ".public_symbol_map = ts_symbol_map,");
        add_line!(self, ".alias_map = ts_non_terminal_alias_map,");
        if !self.parse_table.production_infos.is_empty() {
            add_line!(self, ".alias_sequences = ts_alias_sequences,");
            add_line!(self, ".alias_sequence_offsets = ts_alias_sequence_offsets,");
            add_line!(
                self,
                ".alias_sequence_count = {},",
                self.alias_sequence_count
            );
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
            *next_parse_action_list_index += pool.get(id).len().max(1) as u32;
            result
        }
    }

    fn external_token_id(&self, token_idx: usize) -> String {
        self.external_token_ids[token_idx].clone()
    }

    fn assign_symbol_id(
        &mut self,
        symbol: Symbol,
        used_identifiers: &mut FxHashSet<String>,
        suffixes: &mut FxHashMap<String, usize>,
    ) {
        let id = if symbol == Symbol::end() {
            "ts_builtin_sym_end".to_string()
        } else {
            let (name, kind) = self.metadata_for_symbol(symbol);
            match kind {
                VariableType::Auxiliary => format!("aux_sym_{}", self.sanitize_identifier(name)),
                VariableType::Anonymous => format!("anon_sym_{}", self.sanitize_identifier(name)),
                VariableType::Hidden | VariableType::Named => {
                    format!("sym_{}", self.sanitize_identifier(name))
                }
            }
        };
        let id = Self::unique_identifier(id, used_identifiers, suffixes);
        self.symbol_ids.insert(symbol, id);
    }

    fn unique_identifier(
        mut id: String,
        used_identifiers: &mut FxHashSet<String>,
        suffixes: &mut FxHashMap<String, usize>,
    ) -> String {
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
        used_identifiers.insert(id.clone());
        id
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
    semantic_version: Option<(u8, u8, u8)>,
    supertype_symbol_map: BTreeMap<Symbol, Vec<ChildType>>,
    profile: Option<&GenerationProfile>,
) -> RenderResult<String> {
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
