use std::{
    cmp,
    collections::{BTreeMap, BTreeSet},
    fmt::{Display, Write},
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
    lex_table_pack::{can_use_direct_keyword_lookup, pack_lex_table},
    node_types::ChildType,
    rules::Alias,
    rules::{AliasMap, Symbol, SymbolType, TokenSet},
    strpool::{StrId, StrPool},
    tables::{GotoAction, LexTable, ParseAction, ParseTable},
};

const BITMAP_STATE_MIN_ENTRIES: usize = 16;
/// The largest count that fits in the 7 bits of a packed action list header.
const MAX_ACTION_LIST_LENGTH: usize = 127;
/// The largest action-list start offset that fits in the 15 bits of a terminal parse-table cell.
const MAX_ACTION_LIST_OFFSET: usize = 0x7fff;
/// The largest reduce-record start offset that fits in the 16 bits of a packed `REDUCE(record)` word.
const MAX_REDUCE_RECORD_OFFSET: usize = u16::MAX as usize;
const MAX_GROUPED_LOOKUP_STEPS: usize = 16;
const PARSE_TABLE_REUSABLE_FLAG: u32 = 0x8000;
/// Marks a map entry that refers to a descriptor, which `add_descriptor` lays out.
const SMALL_STATE_DESCRIPTOR_FLAG: usize = 0x2000_0000;
/// Marks the diff entry of a descriptor that ends with a base row. Row entries never set it otherwise.
const SMALL_STATE_BASE_FLAG: usize = SMALL_STATE_DESCRIPTOR_FLAG;
/// The words of a descriptor with a base row.
const BASED_DESCRIPTOR_WORDS: usize = 6;
const UNBASED_DESCRIPTOR_WORDS: usize = 4;
const SMALL_STATE_BITMAP_FLAG: usize = 0x4000_0000;
const SMALL_STATE_DEFAULT_GOTO_FLAG: usize = SMALL_STATE_DESCRIPTOR_FLAG | SMALL_STATE_BITMAP_FLAG;
const SMALL_STATE_PAIR_FLAG: usize = 0x8000_0000;
const SMALL_STATE_SET_FLAG: usize = SMALL_STATE_BITMAP_FLAG | SMALL_STATE_PAIR_FLAG;

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
    #[error("Parse action list offset {0} exceeds maximum value of {MAX_ACTION_LIST_OFFSET}")]
    ActionListOffset(usize),
    #[error("Reduce record offset {0} exceeds maximum value of {MAX_REDUCE_RECORD_OFFSET}")]
    ReduceRecordOffset(usize),
    #[error("Parse action list length {0} exceeds maximum value of {MAX_ACTION_LIST_LENGTH}")]
    ActionListLength(usize),
    #[error("Lexer table error: {0}")]
    LexTable(String),
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

/// Plain rows of one half of the sparse table that described rows can use as bases, with their entries as
/// `row_entries` returns them.
#[derive(Default)]
struct RowBases {
    rows: Vec<(usize, Vec<(usize, u32)>)>,
    rows_by_entry: FxHashMap<(usize, u32), Vec<u32>>,
    offsets: FxHashSet<usize>,
}

impl RowBases {
    fn add(&mut self, offset: usize, entries: Vec<(usize, u32)>) {
        if !self.offsets.insert(offset) {
            return;
        }
        let id = self.rows.len() as u32;
        for &entry in &entries {
            self.rows_by_entry.entry(entry).or_default().push(id);
        }
        self.rows.push((offset, entries));
    }
}

#[derive(Default)]
struct SmallTables {
    next_index: usize,
    rows: Vec<SmallParseRow>,
    row_offsets: FxHashMap<Vec<(usize, u32)>, usize>,
    symbol_sets: FxHashMap<Vec<u16>, usize>,
    bitmap_headers: FxHashMap<Vec<u16>, usize>,
    descriptors: FxHashMap<Vec<u16>, usize>,
    /// Terminal rows, then goto rows.
    bases: [RowBases; 2],
}

#[derive(Default)]
struct Generator {
    buffer: String,
    indent_level: usize,
    language_name: String,
    parse_table: ParseTable,
    main_lex_table: LexTable,
    keyword_lex_table: LexTable,
    large_state_count: usize,
    token_count: usize,
    string_data_size: usize,
    /// The most common goto value of each nonterminal, by its position after the tokens.
    goto_defaults: Vec<u32>,
    alias_sequence_count: usize,
    lex_mode_count: usize,
    uses_reserved_word_slices: bool,
    external_state_stride: usize,
    profile: Option<GenerationProfile>,
    syntax_grammar: SyntaxGrammar,
    lexical_grammar: LexicalGrammar,
    default_aliases: AliasMap,
    symbol_order: FxHashMap<Symbol, usize>,
    symbols_by_order: Vec<Symbol>,
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
        if !self.field_names.is_empty() {
            self.add_field_name_enum();
        }
        self.add_string_tables();
        self.add_unique_symbol_map();
        self.add_symbol_metadata_list();

        if !self.field_names.is_empty() {
            self.add_field_sequences();
        }

        if !self.parse_table.production_infos.is_empty() {
            self.add_alias_sequences();
        }

        if !self.supertype_map.is_empty() {
            self.add_supertype_map();
        }

        self.add_lex_section()?;
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

    fn add_lex_section(&mut self) -> RenderResult<()> {
        let numeric_symbol_ids = self
            .symbol_order
            .iter()
            .map(|(&symbol, &order)| (symbol, order as u16))
            .collect::<FxHashMap<_, _>>();

        let mut main_lex_table = LexTable::default();
        swap(&mut main_lex_table, &mut self.main_lex_table);
        let packed_main = pack_lex_table(&main_lex_table, &numeric_symbol_ids, false)?;
        drop(main_lex_table);
        self.add_integer_array("static const uint16_t ts_lex_table[]", &packed_main);

        if self.syntax_grammar.word_token.is_some() {
            let mut keyword_lex_table = LexTable::default();
            swap(&mut keyword_lex_table, &mut self.keyword_lex_table);
            let allow_direct = can_use_direct_keyword_lookup(
                &self.syntax_grammar,
                &self.lexical_grammar,
                &keyword_lex_table,
            );
            let packed_kw = pack_lex_table(&keyword_lex_table, &numeric_symbol_ids, allow_direct)?;
            self.add_integer_array("static const uint16_t ts_keyword_lex_table[]", &packed_kw);
        }
        Ok(())
    }

    // code-gauge-allow functionCognitiveComplexity -- validates profile invariants and permutes parse and lex states together
    // code-gauge-allow functionNcss -- validates profile invariants and permutes parse and lex states together
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

    // code-gauge-allow functionCognitiveComplexity -- derives every per-symbol table the renderer reads, in dependency order
    // code-gauge-allow functionNcss -- derives every per-symbol table the renderer reads, in dependency order
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
        self.symbols_by_order = vec![Symbol::end()];
        for (i, symbol) in self
            .parse_table
            .symbols
            .iter()
            .filter(|symbol| **symbol != Symbol::end())
            .enumerate()
        {
            self.symbol_order.insert(*symbol, i + 1);
            self.symbols_by_order.push(*symbol);
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

    // code-gauge-allow functionNcss -- computes canonical public symbol mappings across aliases, supertypes, and terminals
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

    fn count_large_states(&self) -> usize {
        if let Some(profile) = &self.profile {
            return Self::count_profiled_large_states(profile);
        }
        let minimum = self.parse_table.states.len().min(2);
        let dense_bytes = self.token_count * size_of::<u16>();
        let (rows, mut row_counts, row_bytes) = self.unprofiled_terminal_rows(minimum);
        let map_bytes = size_of::<u32>();
        let mut bytes =
            minimum * dense_bytes + rows.len() * map_bytes + row_bytes.iter().sum::<usize>();
        let mut best_bytes = bytes;
        let mut count = minimum;
        for (i, &id) in rows.iter().enumerate() {
            bytes = bytes + dense_bytes - map_bytes;
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

    fn count_profiled_large_states(profile: &GenerationProfile) -> usize {
        let total = profile
            .parse_states
            .iter()
            .map(|&n| u128::from(n))
            .sum::<u128>();
        let mut count = 2;
        let mut covered = u128::from(profile.parse_states[0]) + u128::from(profile.parse_states[1]);
        while count < profile.max_dense_states && covered * 10 < total * 9 {
            covered += u128::from(profile.parse_states[count]);
            count += 1;
        }
        count
    }

    fn unprofiled_terminal_rows(&self, minimum: usize) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
        let mut groups = FxHashMap::default();
        let mut row_ids = FxHashMap::default();
        let mut rows = Vec::new();
        let mut row_counts = Vec::new();
        let mut row_bytes = Vec::new();
        for state in self.parse_table.states.iter().skip(minimum) {
            let mut row = state
                .terminal_entries
                .iter()
                .map(|(&symbol, id)| {
                    (
                        self.symbol_order[&symbol],
                        (id.index() as u32) * 2 + u32::from(id.reusable()),
                    )
                })
                .collect::<Vec<_>>();
            row.sort_unstable_by_key(|&(order, _)| order);
            let next_id = row_counts.len();
            let id = *row_ids.entry(row).or_insert_with_key(|row| {
                row_counts.push(0usize);
                groups.clear();
                for &(_, value) in row {
                    *groups.entry(value).or_insert(0usize) += 1;
                }
                let span_blocks = row
                    .first()
                    .zip(row.last())
                    .map_or(0, |(&(first, _), &(last, _))| last / 64 - first / 64 + 1);
                let (_, words) =
                    Self::small_state_encoding(groups.values().copied(), span_blocks, false);
                row_bytes.push(words * size_of::<u16>());
                next_id
            });
            row_counts[id] += 1;
            rows.push(id);
        }
        (rows, row_counts, row_bytes)
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

    fn add_string_tables(&mut self) {
        let (string_data, symbol_offsets, field_offsets) = self.build_string_tables();
        self.string_data_size = string_data.len();
        self.add_integer_array("static const uint8_t ts_string_data[]", &string_data);
        self.add_designated_array("uint32_t", "ts_symbol_name_offsets", symbol_offsets);

        if !field_offsets.is_empty() {
            let entries = field_offsets
                .into_iter()
                .enumerate()
                .map(|(ix, offset)| {
                    let name = self.str_pool.resolve(self.field_names[ix]);
                    (format!("{} - 1", Self::field_id(name)), offset)
                })
                .collect::<Vec<_>>();
            self.add_designated_array("uint32_t", "ts_field_name_offsets", entries);
        }
    }

    fn add_designated_array<K: Display, V: Display>(
        &mut self,
        c_type: &str,
        name: &str,
        entries: impl IntoIterator<Item = (K, V)>,
    ) {
        add_line!(self, "static const {c_type} {name}[] = {{");
        indent!(self);
        for (key, value) in entries {
            add_line!(self, "[{key}] = {value},");
        }
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "");
    }

    fn build_string_tables(&self) -> (Vec<u8>, Vec<(String, u32)>, Vec<u32>) {
        let mut string_data = Vec::<u8>::new();
        let mut offsets_by_str = FxHashMap::<Vec<u8>, u32>::default();
        let mut intern = |raw: &str| -> u32 {
            let trimmed = raw.as_bytes().split(|&b| b == 0).next().unwrap_or(&[]);
            let mut key = Vec::with_capacity(trimmed.len() + 1);
            key.extend_from_slice(trimmed);
            key.push(0);
            if let Some(&offset) = offsets_by_str.get(&key) {
                return offset;
            }
            let offset = string_data.len() as u32;
            string_data.extend_from_slice(&key);
            offsets_by_str.insert(key, offset);
            offset
        };
        let symbol_offsets = self
            .parse_table
            .symbols
            .iter()
            .map(|symbol| {
                let name_id = self
                    .default_aliases
                    .get(symbol)
                    .map_or_else(|| self.metadata_for_symbol(*symbol).0, |alias| alias.value);
                (self.symbol_ids[symbol].clone(), name_id)
            })
            .chain(
                self.unique_aliases
                    .iter()
                    .map(|alias| (self.alias_ids[alias].clone(), alias.value)),
            )
            .map(|(id, name_id)| (id, intern(self.str_pool.resolve(name_id))))
            .collect::<Vec<_>>();
        let field_offsets = self
            .field_names
            .iter()
            .map(|&field| intern(self.str_pool.resolve(field)))
            .collect::<Vec<_>>();
        (string_data, symbol_offsets, field_offsets)
    }

    fn add_unique_symbol_map(&mut self) {
        let entries = self.symbol_and_alias_entries(
            |symbol| self.symbol_ids[&self.symbol_map[&symbol]].clone(),
            |alias| self.alias_ids[alias].clone(),
        );
        self.add_designated_array("TSSymbol", "ts_symbol_map", entries);
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

    fn add_symbol_metadata_list(&mut self) {
        let entries = self.symbol_and_alias_entries(
            |symbol| self.symbol_metadata_flags(symbol),
            |alias| 0x01u8 | if alias.is_named { 0x02 } else { 0 },
        );
        self.add_designated_array("uint8_t", "ts_symbol_metadata", entries);
    }

    fn symbol_and_alias_entries<V>(
        &self,
        on_symbol: impl Fn(Symbol) -> V,
        on_alias: impl Fn(&Alias) -> V,
    ) -> Vec<(String, V)> {
        self.parse_table
            .symbols
            .iter()
            .map(|&symbol| (self.symbol_ids[&symbol].clone(), on_symbol(symbol)))
            .chain(
                self.unique_aliases
                    .iter()
                    .map(|alias| (self.alias_ids[alias].clone(), on_alias(alias))),
            )
            .collect()
    }

    fn symbol_metadata_flags(&self, symbol: Symbol) -> u8 {
        let (visible, named, supertype) = if self
            .syntax_grammar
            .supertype_alias(symbol, &self.default_aliases)
            .is_some()
        {
            (false, true, true)
        } else if let Some(Alias { is_named, .. }) = self.default_aliases.get(&symbol) {
            (true, *is_named, false)
        } else {
            match self.metadata_for_symbol(symbol).1 {
                VariableType::Named => (true, true, false),
                VariableType::Anonymous => (true, false, false),
                VariableType::Hidden => (
                    false,
                    true,
                    self.syntax_grammar.supertype_symbols.contains(&symbol),
                ),
                VariableType::Auxiliary => (false, false, false),
            }
        };
        let immediate = symbol.is_terminal()
            && self.lexical_grammar.variables[symbol.index as usize].is_immediate;
        u8::from(visible)
            | (u8::from(named) << 1)
            | (u8::from(supertype) << 2)
            | (u8::from(immediate) << 3)
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

    // code-gauge-allow functionCognitiveComplexity -- deduplicates field map rows and emits slices and entries together
    // code-gauge-allow functionNcss -- deduplicates field map rows and emits slices and entries together
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

    // code-gauge-allow functionNcss -- resolves aliased and normal subtype sets and emits supertype slices and entries
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
        let id_width = if modes.len() <= 256 { 1 } else { 2 };
        if modes.len() * 10 + ids.len() * id_width >= ids.len() * 10 {
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
        self.add_integer_array(
            &format!(
                "static const uint{}_t ts_lex_mode_ids[STATE_COUNT]",
                id_width * 8
            ),
            &ids,
        );
        true
    }

    fn add_integer_array<T: std::fmt::Display>(&mut self, declaration: &str, values: &[T]) {
        add_line!(self, "{declaration} = {{");
        indent!(self);
        for chunk in values.chunks(16) {
            add_whitespace!(self);
            for value in chunk {
                add!(self, "{value}, ");
            }
            add!(self, "\n");
        }
        dedent!(self);
        add_line!(self, "}};\n");
    }

    // code-gauge-allow functionNcss -- chooses between flat and 2D reserved-word tables and emits the smaller representation
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
        if u8::try_from(stride).is_ok()
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
        if self.large_state_count == 0 || self.token_count == 0 {
            return;
        }
        add_line!(
            self,
            "static const uint16_t ts_parse_table[LARGE_STATE_COUNT][TOKEN_COUNT] = {{",
        );
        indent!(self);
        let mut terminal_entries = Vec::new();
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
            terminal_entries.extend(state.terminal_entries.iter());
            terminal_entries.sort_unstable_by_key(|e| self.symbol_order.get(e.0));
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
        if !self.has_small_parse_table() {
            return Ok(());
        }
        let nonterm_count = self.parse_table.symbols.len() - self.token_count;
        let (non_empty_goto_states, dir_words, goto_map_index, next_index) =
            self.reserve_goto_directory(nonterm_count)?;
        let mut tables = SmallTables {
            next_index,
            ..SmallTables::default()
        };
        let (terminal_map, goto_by_state) = self.populate_small_parse_rows(
            nonterm_count,
            parse_table_entries,
            next_parse_action_list_index,
            &mut tables,
        )?;
        let goto_map_words = non_empty_goto_states
            .into_iter()
            .flat_map(|state_id| {
                let entry = goto_by_state[state_id];
                [(entry & 0xffff) as u16, (entry >> 16) as u16]
            })
            .collect::<Vec<_>>();
        self.emit_small_parse_table_rows(
            nonterm_count,
            dir_words,
            goto_map_index,
            goto_map_words,
            tables.rows,
        );
        self.emit_small_parse_table_map(&terminal_map);
        Ok(())
    }

    fn reserve_goto_directory(
        &self,
        nonterm_count: usize,
    ) -> RenderResult<(Vec<usize>, Vec<u16>, usize, usize)> {
        if nonterm_count == 0 {
            return Ok((Vec::new(), Vec::new(), 0, 0));
        }
        let state_count = self.parse_table.states.len();
        let dir_blocks = state_count.div_ceil(64);
        let mut non_empty_goto_states = Vec::new();
        let mut dir_words = Vec::with_capacity(dir_blocks * 6);
        let mut rank = 0u32;
        for block in 0..dir_blocks {
            let start = block * 64;
            let end = (start + 64).min(state_count);
            let mut bits = 0u64;
            for (offset, state) in self.parse_table.states[start..end].iter().enumerate() {
                if !state.nonterminal_entries.is_empty() {
                    bits |= 1u64 << offset;
                    non_empty_goto_states.push(start + offset);
                }
            }
            dir_words.extend([
                bits as u16,
                (bits >> 16) as u16,
                (bits >> 32) as u16,
                (bits >> 48) as u16,
                (rank & 0xffff) as u16,
                (rank >> 16) as u16,
            ]);
            rank += bits.count_ones();
        }
        let goto_map_index = nonterm_count + dir_words.len();
        let next_index = goto_map_index + non_empty_goto_states.len() * 2;
        if next_index >= SMALL_STATE_DESCRIPTOR_FLAG {
            return Err(RenderError::SparseTable(next_index));
        }
        Ok((non_empty_goto_states, dir_words, goto_map_index, next_index))
    }

    fn populate_small_parse_rows(
        &self,
        nonterm_count: usize,
        parse_table_entries: &mut FxHashMap<ActionListId, u32>,
        next_parse_action_list_index: &mut u32,
        tables: &mut SmallTables,
    ) -> RenderResult<(Vec<usize>, Vec<usize>)> {
        let state_count = self.parse_table.states.len();
        let hot = self.hot_parse_states();
        let is_cold = |state_id: usize| {
            hot.as_ref()
                .map_or(state_id >= self.large_state_count, |hot| !hot[state_id])
        };
        let mut state_ids = (0..state_count).collect::<Vec<_>>();
        state_ids.sort_by_key(|&state_id| is_cold(state_id));
        let mut terminal_map = vec![0usize; state_count - self.large_state_count];
        let mut goto_by_state = vec![0usize; state_count];
        for state_id in state_ids {
            let described = is_cold(state_id);
            let minimize = hot.is_some() && described;
            let (terminal_groups, goto_groups) = self.small_state_groups(
                state_id,
                parse_table_entries,
                next_parse_action_list_index,
            );
            if state_id >= self.large_state_count {
                terminal_map[state_id - self.large_state_count] =
                    self.add_small_terminal_row(&terminal_groups, minimize, described, tables)?;
            }
            if nonterm_count > 0
                && !self.parse_table.states[state_id]
                    .nonterminal_entries
                    .is_empty()
            {
                goto_by_state[state_id] =
                    self.add_small_goto_row(&goto_groups, minimize, described, tables)?;
            }
        }
        Ok((terminal_map, goto_by_state))
    }

    fn hot_parse_states(&self) -> Option<Vec<bool>> {
        let profile = self.profile.as_ref()?;
        let total = profile
            .parse_states
            .iter()
            .map(|&n| u128::from(n))
            .sum::<u128>();
        if total == 0 {
            return None;
        }
        let mut order = (0..profile.parse_states.len()).collect::<Vec<_>>();
        order.sort_unstable_by_key(|&i| (cmp::Reverse(profile.parse_states[i]), i));
        let mut hot = vec![false; profile.parse_states.len()];
        for item in hot.iter_mut().take(self.large_state_count) {
            *item = true;
        }
        let mut covered = 0u128;
        for state_id in order {
            let visits = u128::from(profile.parse_states[state_id]);
            if visits == 0 || covered * 100 >= total * 99 {
                break;
            }
            hot[state_id] = true;
            covered += visits;
        }
        Some(hot)
    }

    fn emit_small_parse_table_rows(
        &mut self,
        nonterm_count: usize,
        dir_words: Vec<u16>,
        goto_map_index: usize,
        goto_map_words: Vec<u16>,
        mut rows: Vec<SmallParseRow>,
    ) {
        add_line!(self, "static const uint16_t ts_small_parse_table[] = {{");
        indent!(self);
        if nonterm_count > 0 {
            let defaults = SmallParseRow {
                index: 0,
                values: self.goto_defaults.iter().map(|&v| v as u16).collect(),
            };
            self.add_small_parse_row(&defaults);
            if !dir_words.is_empty() {
                self.add_small_parse_row(&SmallParseRow {
                    index: nonterm_count,
                    values: dir_words,
                });
            }
            if !goto_map_words.is_empty() {
                self.add_small_parse_row(&SmallParseRow {
                    index: goto_map_index,
                    values: goto_map_words,
                });
            }
        }
        rows.sort_unstable_by_key(|row| row.index);
        for row in &rows {
            self.add_small_parse_row(row);
        }
        dedent!(self);
        add_line!(self, "}};\n");
    }

    fn emit_small_parse_table_map(&mut self, terminal_map: &[usize]) {
        if terminal_map.is_empty() {
            return;
        }
        add_line!(
            self,
            "static const uint32_t ts_small_parse_table_map[] = {{"
        );
        indent!(self);
        for (idx, &offset) in terminal_map.iter().enumerate() {
            let state = self.large_state_count + idx;
            add_line!(self, "[SMALL_STATE({state})] = {offset},");
        }
        dedent!(self);
        add_line!(self, "}};\n");
    }

    /// Adds a state's terminal row, as a described row when that takes fewer words, and returns its map entry.
    fn add_small_terminal_row(
        &self,
        groups: &SmallStateGroups,
        minimize: bool,
        described: bool,
        tables: &mut SmallTables,
    ) -> RenderResult<usize> {
        let entries = self.row_entries(groups);
        if let Some(&offset) = tables.row_offsets.get(&entries) {
            return Ok(offset);
        }
        if described {
            let plain_words = self.plain_row_words(groups, &entries, minimize, tables);
            let base_value = |_: usize| 0;
            let delta = Self::best_delta(&tables.bases[0], &entries, &entries, base_value);
            if let Some((base, diff)) = delta {
                let set = self.order_set(entries.iter().map(|&(order, _)| order));
                let set_words = if tables.symbol_sets.contains_key(&set) {
                    0
                } else {
                    set.len()
                };
                let diff_words = if tables.row_offsets.contains_key(&diff) {
                    0
                } else {
                    Self::delta_words(&diff)
                };
                if diff_words + BASED_DESCRIPTOR_WORDS + set_words < plain_words {
                    let base = tables.bases[0].rows[base].0;
                    return self.add_described_row(set, &diff, base, SymbolType::Terminal, tables);
                }
            }
        }
        let offset = self.add_plain_row(groups, &entries, minimize, tables)?;
        tables.bases[0].add(offset, entries);
        Ok(offset)
    }

    /// Adds a state's goto row and returns its map entry.
    fn add_small_goto_row(
        &self,
        groups: &SmallStateGroups,
        minimize: bool,
        described: bool,
        tables: &mut SmallTables,
    ) -> RenderResult<usize> {
        let entries = self.row_entries(groups);
        let default_value = |order: usize| self.goto_defaults[order - self.token_count];
        let exceptions = entries
            .iter()
            .copied()
            .filter(|&(order, value)| default_value(order) != value)
            .collect::<Vec<_>>();
        let set = self.order_set(entries.iter().map(|&(order, _)| order));
        if exceptions.is_empty() {
            let set_offset = Self::add_shared_values(
                set,
                &mut tables.symbol_sets,
                &mut tables.rows,
                &mut tables.next_index,
            )?;
            return Ok(set_offset | SMALL_STATE_DEFAULT_GOTO_FLAG);
        }
        if !described {
            let offset = self.add_plain_row(groups, &entries, minimize, tables)?;
            tables.bases[1].add(offset, entries);
            return Ok(offset);
        }
        if let Some(&offset) = tables.row_offsets.get(&entries) {
            return Ok(offset);
        }
        self.add_described_goto_or_plain(groups, entries, exceptions, set, minimize, tables)
    }

    fn add_described_goto_or_plain(
        &self,
        groups: &SmallStateGroups,
        entries: Vec<(usize, u32)>,
        exceptions: Vec<(usize, u32)>,
        set: Vec<u16>,
        minimize: bool,
        tables: &mut SmallTables,
    ) -> RenderResult<usize> {
        let plain_words = self.plain_row_words(groups, &entries, minimize, tables);
        let set_words = if tables.symbol_sets.contains_key(&set) {
            0
        } else {
            set.len()
        };
        let exc_words = if tables.row_offsets.contains_key(&exceptions) {
            0
        } else {
            Self::delta_words(&exceptions)
        };
        let unbased_cost = UNBASED_DESCRIPTOR_WORDS + set_words + exc_words;
        let default_value = |order: usize| self.goto_defaults[order - self.token_count];
        if let Some((base, diff)) =
            Self::best_delta(&tables.bases[1], &entries, &exceptions, default_value)
        {
            let diff_words = if tables.row_offsets.contains_key(&diff) {
                0
            } else {
                Self::delta_words(&diff)
            };
            let based_cost = BASED_DESCRIPTOR_WORDS + set_words + diff_words;
            if based_cost < unbased_cost && based_cost < plain_words {
                let base_offset = tables.bases[1].rows[base].0;
                return self.add_described_row(
                    set,
                    &diff,
                    base_offset,
                    SymbolType::NonTerminal,
                    tables,
                );
            }
        }
        if unbased_cost < plain_words {
            let exc_groups = self.entry_groups(&exceptions, SymbolType::NonTerminal);
            let exc_offset = self.add_plain_row(&exc_groups, &exceptions, minimize, tables)?;
            tables.bases[1].add(exc_offset, exceptions);
            return Self::add_descriptor(set, exc_offset, None, tables);
        }
        let offset = self.add_plain_row(groups, &entries, minimize, tables)?;
        tables.bases[1].add(offset, entries);
        Ok(offset)
    }

    fn add_described_row(
        &self,
        set: Vec<u16>,
        diff: &[(usize, u32)],
        base: usize,
        kind: SymbolType,
        tables: &mut SmallTables,
    ) -> RenderResult<usize> {
        let groups = self.entry_groups(diff, kind);
        let diff_offset = self.add_plain_row(&groups, diff, true, tables)?;
        Self::add_descriptor(set, diff_offset, Some(base), tables)
    }

    fn add_descriptor(
        set: Vec<u16>,
        diff: usize,
        base: Option<usize>,
        tables: &mut SmallTables,
    ) -> RenderResult<usize> {
        let set_offset = Self::add_shared_values(
            set,
            &mut tables.symbol_sets,
            &mut tables.rows,
            &mut tables.next_index,
        )?;
        let diff = if base.is_some() {
            diff | SMALL_STATE_BASE_FLAG
        } else {
            diff
        };
        let descriptor = [set_offset, diff]
            .into_iter()
            .chain(base)
            .flat_map(|offset| [(offset & 0xffff) as u16, (offset >> 16) as u16])
            .collect();
        let offset = Self::add_shared_values(
            descriptor,
            &mut tables.descriptors,
            &mut tables.rows,
            &mut tables.next_index,
        )?;
        Ok(offset | SMALL_STATE_DESCRIPTOR_FLAG)
    }

    fn add_plain_row(
        &self,
        groups: &SmallStateGroups,
        entries: &[(usize, u32)],
        minimize: bool,
        tables: &mut SmallTables,
    ) -> RenderResult<usize> {
        if let Some(&offset) = tables.row_offsets.get(entries) {
            return Ok(offset);
        }
        let offset = self.add_small_parse_rows(groups, entries, minimize, tables)?;
        tables.row_offsets.insert(entries.to_vec(), offset);
        Ok(offset)
    }

    fn plain_row_words(
        &self,
        groups: &SmallStateGroups,
        entries: &[(usize, u32)],
        minimize: bool,
        tables: &SmallTables,
    ) -> usize {
        let bitmap_words = Self::bitmap_row_words(entries, &tables.bitmap_headers);
        let (flag, words) = Self::choose_small_state_encoding(
            groups.iter().map(|(_, symbols)| symbols.len()),
            bitmap_words,
            minimize,
        );
        self.shared_parse_row_words(groups, minimize || flag == 0, &tables.symbol_sets)
            .filter(|&shared_words| shared_words < words)
            .unwrap_or(words)
    }

    fn add_small_parse_rows(
        &self,
        groups: &[((u32, SymbolType), Vec<Symbol>)],
        entries: &[(usize, u32)],
        minimize: bool,
        tables: &mut SmallTables,
    ) -> RenderResult<usize> {
        let header = Self::bitmap_header(entries);
        let bitmap_words = if entries.is_empty() {
            usize::MAX
        } else {
            let hdr_words = if tables.bitmap_headers.contains_key(&header) {
                0
            } else {
                header.len()
            };
            hdr_words + 2 + entries.len()
        };
        let (flag, words) = Self::choose_small_state_encoding(
            groups.iter().map(|(_, symbols)| symbols.len()),
            bitmap_words,
            minimize,
        );
        if let Some(offset) =
            self.encode_shared_parse_row(groups, words, minimize || flag == 0, tables)?
        {
            return Ok(offset);
        }
        if flag == SMALL_STATE_BITMAP_FLAG {
            let hdr_offset = Self::add_shared_values(
                header,
                &mut tables.bitmap_headers,
                &mut tables.rows,
                &mut tables.next_index,
            )?;
            let mut values = Vec::with_capacity(2 + entries.len());
            values.extend([(hdr_offset & 0xffff) as u16, (hdr_offset >> 16) as u16]);
            values.extend(entries.iter().map(|&(_, value)| value as u16));
            return Self::push_row(values, SMALL_STATE_BITMAP_FLAG, tables);
        }
        let values = self.encode_grouped_or_pair_row(groups, entries, flag);
        Self::push_row(values, flag, tables)
    }

    fn push_row(values: Vec<u16>, flag: usize, tables: &mut SmallTables) -> RenderResult<usize> {
        let index = tables.next_index;
        tables.next_index += values.len();
        if tables.next_index >= SMALL_STATE_DESCRIPTOR_FLAG {
            return Err(RenderError::SparseTable(tables.next_index));
        }
        tables.rows.push(SmallParseRow { index, values });
        Ok(index | flag)
    }

    fn bitmap_row_words(
        entries: &[(usize, u32)],
        bitmap_headers: &FxHashMap<Vec<u16>, usize>,
    ) -> usize {
        if entries.is_empty() {
            return usize::MAX;
        }
        let header = Self::bitmap_header(entries);
        let hdr_words = if bitmap_headers.contains_key(&header) {
            0
        } else {
            header.len()
        };
        hdr_words + 2 + entries.len()
    }

    fn bitmap_header(entries: &[(usize, u32)]) -> Vec<u16> {
        let Some((&(first, _), &(last, _))) = entries.first().zip(entries.last()) else {
            return vec![0, 0];
        };
        let first_block = first / 64;
        let block_count = last / 64 - first_block + 1;
        let mut header = vec![0u16; 2 + block_count * 5];
        header[0] = first_block as u16;
        header[1] = block_count as u16;
        for &(order, _) in entries {
            let block = order / 64 - first_block;
            let bit = order % 64;
            header[2 + block * 5 + bit / 16] |= 1u16 << (bit % 16);
        }
        let mut rank = 0u16;
        for block in 0..block_count {
            let base = 2 + block * 5;
            header[base + 4] = rank;
            let ones = header[base].count_ones()
                + header[base + 1].count_ones()
                + header[base + 2].count_ones()
                + header[base + 3].count_ones();
            rank += ones as u16;
        }
        header
    }

    fn encode_shared_parse_row(
        &self,
        groups: &[((u32, SymbolType), Vec<Symbol>)],
        unshared_words: usize,
        allow_many_groups: bool,
        tables: &mut SmallTables,
    ) -> RenderResult<Option<usize>> {
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
                .filter(|set| !tables.symbol_sets.contains_key(*set))
                .map(Vec::len)
                .sum::<usize>();
        if words >= unshared_words {
            return Ok(None);
        }
        let mut offsets = Vec::with_capacity(sets.len());
        for set in sets {
            let offset = Self::add_shared_values(
                set,
                &mut tables.symbol_sets,
                &mut tables.rows,
                &mut tables.next_index,
            )?;
            offsets.push(offset);
        }
        let mut values = Vec::with_capacity(1 + groups.len() * 3);
        values.push(groups.len() as u16);
        for (((value, _), _), set_offset) in groups.iter().zip(offsets) {
            values.extend([
                *value as u16,
                (set_offset & 0xffff) as u16,
                (set_offset >> 16) as u16,
            ]);
        }
        Self::push_row(values, SMALL_STATE_SET_FLAG, tables).map(Some)
    }

    fn shared_parse_row_words(
        &self,
        groups: &[((u32, SymbolType), Vec<Symbol>)],
        allow_many_groups: bool,
        symbol_sets: &FxHashMap<Vec<u16>, usize>,
    ) -> Option<usize> {
        if groups.len() > 4 && !allow_many_groups {
            return None;
        }
        let set_words = groups
            .iter()
            .map(|(_, symbols)| self.symbol_set(symbols))
            .filter(|set| !symbol_sets.contains_key(set))
            .map(|set| set.len())
            .sum::<usize>();
        Some(1 + groups.len() * 3 + set_words)
    }

    fn encode_grouped_or_pair_row(
        &self,
        groups: &[((u32, SymbolType), Vec<Symbol>)],
        entries: &[(usize, u32)],
        flag: usize,
    ) -> Vec<u16> {
        if flag == SMALL_STATE_PAIR_FLAG {
            let mut row = Vec::with_capacity(1 + entries.len() * 2);
            row.push(entries.len() as u16);
            for &(symbol, value) in entries {
                row.extend([symbol as u16, value as u16]);
            }
            return row;
        }
        let mut row = vec![groups.len() as u16];
        for ((value, _), symbols) in groups {
            row.extend([*value as u16, symbols.len() as u16]);
            row.extend(
                symbols
                    .iter()
                    .map(|symbol| self.symbol_order[symbol] as u16),
            );
        }
        row
    }

    fn best_delta(
        bases: &RowBases,
        entries: &[(usize, u32)],
        probes: &[(usize, u32)],
        base_value: impl Fn(usize) -> u32,
    ) -> Option<(usize, Vec<(usize, u32)>)> {
        let mut scores = FxHashMap::<u32, u32>::default();
        for entry in probes {
            for &base in bases
                .rows_by_entry
                .get(entry)
                .into_iter()
                .flatten()
                .rev()
                .take(64)
            {
                *scores.entry(base).or_default() += 1;
            }
        }
        let mut candidates = scores.into_iter().collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|&(base, score)| (cmp::Reverse(score), base));
        candidates
            .into_iter()
            .take(8)
            .map(|(base, _)| {
                let row = &bases.rows[base as usize].1;
                let diff = entries
                    .iter()
                    .copied()
                    .filter(|&(order, value)| {
                        row.binary_search_by_key(&order, |&(order, _)| order)
                            .map_or_else(|_| base_value(order), |i| row[i].1)
                            != value
                    })
                    .collect::<Vec<_>>();
                (base as usize, diff)
            })
            .min_by_key(|(base, diff)| (Self::delta_words(diff), *base))
    }

    fn delta_words(entries: &[(usize, u32)]) -> usize {
        let mut counts = FxHashMap::<u32, usize>::default();
        for &(_, value) in entries {
            *counts.entry(value).or_default() += 1;
        }
        Self::small_state_encoding(counts.into_values(), Self::span_blocks64(entries), true).1
    }

    fn small_state_encoding(
        group_sizes: impl Iterator<Item = usize>,
        span_blocks: usize,
        minimize: bool,
    ) -> (usize, usize) {
        let sizes = group_sizes.collect::<Vec<_>>();
        let entry_count = sizes.iter().sum::<usize>();
        let bitmap_words = if span_blocks == 0 {
            usize::MAX
        } else {
            4 + 5 * span_blocks + entry_count
        };
        Self::choose_small_state_encoding(sizes.into_iter(), bitmap_words, minimize)
    }

    fn choose_small_state_encoding(
        group_sizes: impl Iterator<Item = usize>,
        bitmap_words: usize,
        minimize: bool,
    ) -> (usize, usize) {
        let mut entry_count = 0;
        let mut group_count = 0;
        let mut lookup_steps = 0;
        for size in group_sizes {
            entry_count += size;
            group_count += 1;
            lookup_steps += if size < 16 {
                size
            } else {
                size.ilog2() as usize + 1
            };
        }
        let grouped = (0, 1 + 2 * group_count + entry_count);
        let bitmap = (SMALL_STATE_BITMAP_FLAG, bitmap_words);
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

    fn span_blocks64(entries: &[(usize, u32)]) -> usize {
        entries
            .first()
            .zip(entries.last())
            .map_or(0, |(&(first, _), &(last, _))| last / 64 - first / 64 + 1)
    }

    fn row_entries(&self, groups: &SmallStateGroups) -> Vec<(usize, u32)> {
        let mut entries = groups
            .iter()
            .flat_map(|((value, _), symbols)| {
                symbols
                    .iter()
                    .map(|symbol| (self.symbol_order[symbol], *value))
            })
            .collect::<Vec<_>>();
        entries.sort_unstable();
        entries
    }

    fn entry_groups(&self, entries: &[(usize, u32)], kind: SymbolType) -> SmallStateGroups {
        let mut groups = FxHashMap::<u32, Vec<Symbol>>::default();
        for &(order, value) in entries {
            groups
                .entry(value)
                .or_default()
                .push(self.symbols_by_order[order]);
        }
        let mut groups = groups
            .into_iter()
            .map(|(value, symbols)| ((value, kind), symbols))
            .collect::<Vec<_>>();
        groups.sort_unstable_by_key(|((value, _), symbols)| (symbols.len(), *value, symbols[0]));
        groups
    }

    fn order_set(&self, orders: impl Iterator<Item = usize>) -> Vec<u16> {
        let symbols = orders
            .map(|order| self.symbols_by_order[order])
            .collect::<Vec<_>>();
        if symbols.is_empty() {
            vec![0, 0]
        } else {
            self.symbol_set(&symbols)
        }
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

    fn add_shared_values(
        values: Vec<u16>,
        map: &mut FxHashMap<Vec<u16>, usize>,
        rows: &mut Vec<SmallParseRow>,
        next_index: &mut usize,
    ) -> RenderResult<usize> {
        if let Some(&offset) = map.get(&values) {
            return Ok(offset);
        }
        let index = *next_index;
        *next_index += values.len();
        if *next_index >= SMALL_STATE_DESCRIPTOR_FLAG {
            return Err(RenderError::SparseTable(*next_index));
        }
        map.insert(values.clone(), index);
        rows.push(SmallParseRow { index, values });
        Ok(index)
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
        if state_id >= self.large_state_count {
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
        if let Some(&(max_list_offset, _)) = parse_table_entries.last()
            && max_list_offset as usize > MAX_ACTION_LIST_OFFSET
        {
            return Err(RenderError::ActionListOffset(max_list_offset as usize));
        }
        let reduce_records = self.reduce_records(&parse_table_entries, reduce_records_start);
        if let Some((_, &max_record_offset)) = reduce_records.last()
            && max_record_offset as usize > MAX_REDUCE_RECORD_OFFSET
        {
            return Err(RenderError::ReduceRecordOffset(max_record_offset as usize));
        }

        add_line!(self, "static const uint32_t ts_parse_actions[] = {{");
        indent!(self);
        for (i, id) in parse_table_entries {
            let actions = self.parse_table.action_lists.get(id);
            if actions.len() > MAX_ACTION_LIST_LENGTH {
                return Err(RenderError::ActionListLength(actions.len()));
            }
            let actions = actions
                .iter()
                .map(|action| Self::packed_parse_action(action, &reduce_records))
                .collect::<Vec<_>>();
            add_line!(
                self,
                "[{i}] = ACTION_LIST({}){}{},",
                actions.len(),
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
            ParseAction::Shift { state } => format!("SHIFT({state})"),
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

        self.add_external_scanner_declarations(&external_scanner_name);

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
        self.add_language_table_fields();
        self.add_language_lex_and_scanner_fields(&external_scanner_name);
        dedent!(self);
        add_line!(self, "}};");
        add_line!(self, "return &language;");
        dedent!(self);
        add_line!(self, "}}");
        add_line!(self, "#ifdef __cplusplus");
        add_line!(self, "}}");
        add_line!(self, "#endif");
    }

    fn add_external_scanner_declarations(&mut self, name: &str) {
        if !self.syntax_grammar.external_tokens.is_empty() {
            for decl in [
                format!("void *{name}_create(void);"),
                format!("void {name}_destroy(void *);"),
                format!("bool {name}_scan(void *, TSLexer *, const bool *);"),
                format!("unsigned {name}_serialize(void *, char *);"),
                format!("void {name}_deserialize(void *, const char *, unsigned);"),
            ] {
                add_line!(self, "{decl}");
            }
            add_line!(self, "");
        }
        if self.external_state_stride > 0 {
            add_line!(
                self,
                "static bool ts_external_scanner_scan(void *payload, TSLexer *lexer, const bool *states) {{"
            );
            indent!(self);
            for stmt in [
                "const uint8_t *bits = (const uint8_t *)states;".to_string(),
                "bool valid[EXTERNAL_TOKEN_COUNT];".to_string(),
                "for (unsigned i = 0; i < EXTERNAL_TOKEN_COUNT; i++) valid[i] = (bits[i / 8] >> (i % 8)) & 1;".to_string(),
                format!("return {name}_scan(payload, lexer, valid);"),
            ] {
                add_line!(self, "{stmt}");
            }
            dedent!(self);
            add_line!(self, "}}");
        }
    }

    fn add_language_table_fields(&mut self) {
        add_line!(self, ".abi_version = LANGUAGE_VERSION,");
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
        if self.large_state_count > 0 && self.token_count > 0 {
            add_line!(self, ".parse_table = &ts_parse_table[0][0],");
        }
        if self.has_small_parse_table() {
            add_line!(self, ".small_parse_table = ts_small_parse_table,");
        }
        if self.large_state_count < self.parse_table.states.len() {
            add_line!(self, ".small_parse_table_map = ts_small_parse_table_map,");
        }
        add_line!(self, ".parse_actions = ts_parse_actions,");
        self.add_language_metadata_fields();
    }

    fn add_language_metadata_fields(&mut self) {
        add_line!(self, ".string_data = (const char *)ts_string_data,");
        add_line!(self, ".string_data_size = {},", self.string_data_size);
        add_line!(self, ".symbol_name_offsets = ts_symbol_name_offsets,");
        if !self.field_names.is_empty() {
            add_line!(self, ".field_name_offsets = ts_field_name_offsets,");
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
        if !self.parse_table.production_infos.is_empty() {
            add_line!(self, ".alias_sequences = ts_alias_sequences,");
            add_line!(self, ".alias_sequence_offsets = ts_alias_sequence_offsets,");
            add_line!(
                self,
                ".alias_sequence_count = {},",
                self.alias_sequence_count
            );
        }
    }

    fn add_language_lex_and_scanner_fields(&mut self, external_scanner_name: &str) {
        add_line!(self, ".lex_modes = (const void *)ts_lex_modes,");
        if self.lex_mode_count > 0 {
            add_line!(self, ".lex_mode_ids = ts_lex_mode_ids,");
            add_line!(self, ".lex_mode_count = {},", self.lex_mode_count);
        }
        add_line!(self, ".lex_table = ts_lex_table,");
        if let Some(keyword_capture_token) = self.syntax_grammar.word_token {
            add_line!(self, ".keyword_lex_table = ts_keyword_lex_table,");
            add_line!(
                self,
                ".keyword_capture_token = {},",
                self.symbol_ids[&keyword_capture_token]
            );
        }
        self.add_language_external_and_reserved_fields(external_scanner_name);
    }

    fn add_language_external_and_reserved_fields(&mut self, external_scanner_name: &str) {
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
        self.add_language_reserved_and_metadata_fields();
    }

    fn add_language_reserved_and_metadata_fields(&mut self) {
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
            ".max_reserved_word_set_size = MAX_RESERVED_WORD_SET_SIZE,"
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

    const fn has_small_parse_table(&self) -> bool {
        self.parse_table.symbols.len() > self.token_count
            || self.large_state_count < self.parse_table.states.len()
    }

    fn get_parse_action_list_id(
        id: ActionListId,
        pool: &ActionListPool,
        parse_action_list_offsets: &mut FxHashMap<ActionListId, u32>,
        next_parse_action_list_index: &mut u32,
    ) -> u32 {
        let key = ActionListId::new(id.index() as u32, false);
        let actions = pool.get(key);
        let offset = if let Some(&index) = parse_action_list_offsets.get(&key) {
            index
        } else {
            let result = *next_parse_action_list_index;
            parse_action_list_offsets.insert(key, result);
            *next_parse_action_list_index += actions.len().max(1) as u32;
            result
        };
        if id.reusable() && !actions.is_empty() {
            offset | PARSE_TABLE_REUSABLE_FLAG
        } else {
            offset
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
}

/// Returns a String of C code for the given components of a parser.
#[expect(
    clippy::too_many_arguments,
    reason = "all parameters are required for code generation"
)]
// code-gauge-allow functionParameterCount -- public generator entry point takes pre-built grammar components
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
