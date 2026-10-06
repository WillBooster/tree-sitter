use std::{
    collections::{VecDeque, hash_map::Entry},
    mem,
};

use rustc_hash::{FxHashMap, FxHashSet};

use log::debug;

use super::{
    SymbolIndexer, coincident_tokens::CoincidentTokenIndex, token_conflicts::TokenConflictMap,
};
use crate::{
    dedup::{SplitCriterion, split_state_id_groups},
    grammars::{LexicalGrammar, SyntaxGrammar},
    nfa::{CharacterSet, NfaCursor, NfaState},
    rules::{Symbol, TokenSet},
    strpool::StrPool,
    tables::{
        AdvanceAction, LexState, LexStateId, LexTable, ParseAction, ParseStateId, ParseTable,
    },
};

pub const LARGE_CHARACTER_RANGE_COUNT: usize = 8;

pub struct LexTables {
    pub main_lex_table: LexTable,
    pub keyword_lex_table: LexTable,
    pub large_character_sets: Vec<(Option<Symbol>, CharacterSet)>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "all parameters are required for lex table building"
)]
pub fn build_lex_table(
    parse_table: &mut ParseTable,
    syntax_grammar: &SyntaxGrammar,
    lexical_grammar: &LexicalGrammar,
    keywords: &TokenSet,
    coincident_token_index: &CoincidentTokenIndex,
    token_conflict_map: &TokenConflictMap,
    unsafe_keyword_pairs: &[(Symbol, Symbol)],
    immediate_keywords: &TokenSet,
    str_pool: &StrPool,
) -> LexTables {
    let keyword_lex_table = if syntax_grammar.word_token.is_some() {
        let mut builder = LexTableBuilder::new(lexical_grammar, str_pool);
        builder.add_state_for_tokens(keywords);
        loop {
            minimize_lex_table(&mut builder.table);
            if !coalesce_lex_transitions(&mut builder.table) {
                break;
            }
        }
        builder.table
    } else {
        LexTable::default()
    };

    let mut parse_state_ids_by_token_set = Vec::<(TokenSet, TokenSet, Vec<ParseStateId>)>::new();
    let starting_chars = token_conflict_map.starting_chars();
    let word_start_chars: CharacterSet = syntax_grammar
        .word_token
        .filter(|token| token.is_terminal())
        .map(|w| starting_chars[w.index as usize].clone())
        .unwrap_or_default();
    let mut continuation_cache: FxHashMap<usize, CharacterSet> = FxHashMap::default();
    let token_precedence: Vec<i32> = {
        let mut prec = vec![0i32; lexical_grammar.variables.len()];
        for state in &lexical_grammar.nfa.states {
            if let NfaState::Accept {
                variable_index,
                precedence,
            } = state
            {
                prec[*variable_index] = *precedence;
            }
        }
        prec
    };
    let mut seen_keywords = FxHashSet::default();
    let deferred_keywords: Vec<Symbol> = unsafe_keyword_pairs
        .iter()
        .map(|&(keyword, _)| keyword)
        .chain(immediate_keywords.iter())
        .filter(|&keyword| seen_keywords.insert(keyword))
        .collect();
    let indexer = SymbolIndexer::new(syntax_grammar, lexical_grammar);
    let mut entries_by_symbol = vec![None; indexer.token_count() as usize];
    for (i, state) in parse_table.states.iter().enumerate() {
        for (&symbol, &entry) in state.terminal_entries.iter() {
            entries_by_symbol[indexer.index(symbol)] = Some(entry);
        }
        let has_entry = |symbol| entries_by_symbol[indexer.index(symbol)].is_some();
        let mut retained: Vec<Symbol> = Vec::new();
        if let Some(word_token) = syntax_grammar
            .word_token
            .filter(|token| token.is_terminal())
        {
            for &(pair_kw, other) in unsafe_keyword_pairs {
                if has_entry(pair_kw) && has_entry(other) && !has_entry(word_token) {
                    retained.push(pair_kw);
                    if log::log_enabled!(log::Level::Debug) {
                        debug!(
                            "Keywords - exclude {} in state {} because of conflict with {} (retaining raw keyword)",
                            str_pool
                                .resolve(lexical_grammar.variables[pair_kw.index as usize].name),
                            i,
                            str_pool.resolve(lexical_grammar.variables[other.index as usize].name),
                        );
                    }
                }
            }
            for &pair_kw in &deferred_keywords {
                if retained.contains(&pair_kw)
                    || !has_entry(pair_kw)
                    || (has_entry(word_token)
                        && token_precedence[pair_kw.index as usize]
                            <= token_precedence[word_token.index as usize])
                {
                    continue;
                }
                let keyword_index = pair_kw.index as usize;
                let mut forced = continuation_cache
                    .entry(keyword_index)
                    .or_insert_with(|| {
                        word_continuation_chars(
                            lexical_grammar,
                            word_token.index as usize,
                            keyword_index,
                        )
                    })
                    .clone();
                if !has_entry(word_token) {
                    let mut word_starts = word_start_chars.clone();
                    forced.remove_intersection(&mut word_starts);
                }
                if forced.is_empty() {
                    continue;
                }
                let Some(entry_id) = entries_by_symbol[indexer.index(pair_kw)] else {
                    continue;
                };
                let actions = parse_table.action_lists.get(entry_id);
                let reduction_follows = actions
                    .iter()
                    .any(|action| matches!(action, ParseAction::Reduce { .. }))
                    .then(|| token_conflict_map.following_tokens(keyword_index));
                let culprit = actions
                    .iter()
                    .filter_map(|action| match action {
                        ParseAction::Shift { state: after, .. } => Some(*after),
                        _ => None,
                    })
                    .flat_map(|after| {
                        parse_table.states[after as usize]
                            .terminal_entries
                            .keys()
                            .copied()
                    })
                    .chain(reduction_follows.into_iter().flat_map(TokenSet::iter))
                    .find(|&follow| {
                        follow.is_terminal()
                            && follow != word_token
                            && follow != pair_kw
                            && !keywords.contains(follow)
                            && starting_chars[follow.index as usize].does_intersect_chars(&forced)
                    });
                if let Some(follow) = culprit {
                    retained.push(pair_kw);
                    if log::log_enabled!(log::Level::Debug) {
                        debug!(
                            "Keywords - retain-follow {} in state {} (follow-token {} starts with a word-continuation char; retaining raw keyword)",
                            str_pool.resolve(lexical_grammar.variables[keyword_index].name),
                            i,
                            str_pool.resolve(lexical_grammar.variables[follow.index as usize].name),
                        );
                    }
                }
            }
        }
        let guard_keywords = immediate_keywords
            .iter()
            .filter(|keyword| {
                has_entry(*keyword)
                    && !retained.contains(keyword)
                    && syntax_grammar.word_token.is_some_and(Symbol::is_terminal)
            })
            .collect::<TokenSet>();
        let tokens: TokenSet = state
            .terminal_entries
            .keys()
            .copied()
            .chain(state.reserved_words.iter())
            .filter_map(|token| {
                if token.is_terminal() {
                    let immediate_syntax = lexical_grammar.variables[token.index as usize]
                        .is_immediate
                        && has_entry(token);
                    if i != 0
                        && keywords.contains(token)
                        && !retained.contains(&token)
                        && !immediate_syntax
                    {
                        syntax_grammar.word_token
                    } else {
                        Some(token)
                    }
                } else if token.is_eof() {
                    Some(token)
                } else {
                    None
                }
            })
            .collect();

        let has_terminals = tokens.iter().any(Symbol::is_terminal);
        let mut did_merge = false;
        for entry in &mut parse_state_ids_by_token_set {
            let group_has_terminals = entry.0.iter().any(Symbol::is_terminal);
            let terminal_free = !has_terminals || !group_has_terminals;
            if (terminal_free || entry.1 == guard_keywords)
                && (entry.0 == tokens
                    || ((terminal_free || guard_keywords.is_empty())
                        && merge_token_set(
                            &mut entry.0,
                            &tokens,
                            token_conflict_map,
                            coincident_token_index,
                        )))
            {
                if !group_has_terminals {
                    entry.1 = guard_keywords.clone();
                }
                did_merge = true;
                entry.2.push(i as u32);
                break;
            }
        }

        if !did_merge {
            parse_state_ids_by_token_set.push((tokens, guard_keywords, vec![i as u32]));
        }
        for &symbol in state.terminal_entries.keys() {
            entries_by_symbol[indexer.index(symbol)] = None;
        }
    }

    let mut builder = LexTableBuilder::new(lexical_grammar, str_pool);
    builder.word_token = syntax_grammar
        .word_token
        .filter(|token| token.is_terminal());
    builder.immediate_keywords = immediate_keywords.clone();
    for (tokens, guard_keywords, parse_state_ids) in parse_state_ids_by_token_set {
        let lex_state_id = builder.add_state_for_tokens_with_word_guard(&tokens, &guard_keywords);

        for id in parse_state_ids {
            parse_table.states[id as usize].lex_state_id = lex_state_id;
        }
    }

    let mut main_lex_table = mem::take(&mut builder.table);
    loop {
        let state_map = minimize_lex_table(&mut main_lex_table);
        for state in &mut parse_table.states {
            state.lex_state_id = state_map[state.lex_state_id as usize];
        }
        if !coalesce_lex_transitions(&mut main_lex_table) {
            break;
        }
    }
    sort_states(&mut main_lex_table, parse_table);

    let mut large_character_sets = Vec::new();
    for (variable_ix, _variable) in lexical_grammar.variables.iter().enumerate() {
        let symbol = Symbol::terminal(variable_ix);
        builder.reset();
        builder.add_state_for_tokens(&TokenSet::from_iter([symbol]));
        for state in &builder.table.states {
            let mut characters = CharacterSet::empty();
            for (chars, action) in &state.advance_actions {
                if action.in_main_token {
                    characters = characters.add(chars);
                    continue;
                }

                if chars.range_count() > LARGE_CHARACTER_RANGE_COUNT
                    && !large_character_sets.iter().any(|(_, set)| set == chars)
                {
                    large_character_sets.push((None, chars.clone()));
                }
            }

            if characters.range_count() > LARGE_CHARACTER_RANGE_COUNT
                && !large_character_sets
                    .iter()
                    .any(|(_, set)| *set == characters)
            {
                large_character_sets.push((Some(symbol), characters));
            }
        }
    }

    LexTables {
        main_lex_table,
        keyword_lex_table,
        large_character_sets,
    }
}

fn word_continuation_chars(
    grammar: &LexicalGrammar,
    word_index: usize,
    keyword_index: usize,
) -> CharacterSet {
    let mut w_cursor = NfaCursor::new(&grammar.nfa, Vec::new());
    let mut k_cursor = NfaCursor::new(&grammar.nfa, Vec::new());

    let expand = |cursor: &mut NfaCursor, states: &[u32]| -> Vec<u32> {
        cursor.reset(states.iter().copied());
        cursor.state_ids.clone()
    };

    let mut result = CharacterSet::empty();
    let mut visited: FxHashSet<(Vec<u32>, Vec<u32>)> = FxHashSet::default();
    let mut queue: Vec<(Vec<u32>, Vec<u32>)> = Vec::with_capacity(8);
    queue.push((
        expand(&mut w_cursor, &[grammar.variables[word_index].start_state]),
        expand(
            &mut k_cursor,
            &[grammar.variables[keyword_index].start_state],
        ),
    ));

    while let Some((ws, ks)) = queue.pop() {
        if !visited.insert((ws.clone(), ks.clone())) {
            continue;
        }

        let k_complete = ks.iter().any(|&s| {
            matches!(
                grammar.nfa.states[s as usize],
                NfaState::Accept { variable_index, .. } if variable_index == keyword_index
            )
        });

        w_cursor.reset(ws);
        let w_transitions = w_cursor.transitions();

        if k_complete {
            for t in &w_transitions {
                result = result.add(&t.characters);
            }
        }

        k_cursor.reset(ks);
        let k_transitions = k_cursor.transitions();
        for wt in &w_transitions {
            for kt in &k_transitions {
                let mut wc = wt.characters.clone();
                let mut kc = kt.characters.clone();
                let shared = wc.remove_intersection(&mut kc);
                if shared.is_empty() {
                    continue;
                }
                queue.push((
                    expand(&mut w_cursor, &wt.states),
                    expand(&mut k_cursor, &kt.states),
                ));
            }
        }
    }

    result
}

struct QueueEntry {
    state_id: LexStateId,
    nfa_states: Vec<u32>,
    eof_valid: bool,
    guard_id: usize,
}

struct LexTableBuilder<'a> {
    lexical_grammar: &'a LexicalGrammar,
    str_pool: &'a StrPool,
    word_token: Option<Symbol>,
    immediate_keywords: TokenSet,
    word_guards: Vec<TokenSet>,
    cursor: NfaCursor<'a>,
    table: LexTable,
    state_queue: VecDeque<QueueEntry>,
    state_ids_by_nfa_state_set: FxHashMap<(Vec<u32>, bool, usize), LexStateId>,
}

impl<'a> LexTableBuilder<'a> {
    fn new(lexical_grammar: &'a LexicalGrammar, str_pool: &'a StrPool) -> Self {
        Self {
            lexical_grammar,
            str_pool,
            word_token: None,
            immediate_keywords: TokenSet::new(),
            word_guards: vec![TokenSet::new()],
            cursor: NfaCursor::new(&lexical_grammar.nfa, vec![]),
            table: LexTable::default(),
            state_queue: VecDeque::new(),
            state_ids_by_nfa_state_set: FxHashMap::default(),
        }
    }

    fn reset(&mut self) {
        self.table = LexTable::default();
        self.state_queue.clear();
        self.state_ids_by_nfa_state_set.clear();
    }

    fn add_state_for_tokens(&mut self, tokens: &TokenSet) -> LexStateId {
        self.add_state_for_tokens_with_word_guard(tokens, &TokenSet::new())
    }

    fn add_state_for_tokens_with_word_guard(
        &mut self,
        tokens: &TokenSet,
        guard_keywords: &TokenSet,
    ) -> LexStateId {
        let guard_id = self
            .word_guards
            .iter()
            .position(|keywords| keywords == guard_keywords)
            .unwrap_or_else(|| {
                self.word_guards.push(guard_keywords.clone());
                self.word_guards.len() - 1
            });
        let mut eof_valid = false;
        let mut nfa_states: Vec<u32> = tokens
            .iter()
            .filter_map(|token| {
                if token.is_terminal() {
                    Some(self.lexical_grammar.variables[token.index as usize].start_state)
                } else {
                    eof_valid = true;
                    None
                }
            })
            .collect();
        if guard_id != 0 && !tokens.contains(self.word_token.unwrap()) {
            let word = self.word_token.unwrap();
            self.cursor
                .reset([self.lexical_grammar.variables[word.index as usize].start_state]);
            nfa_states.extend(self.cursor.state_ids.iter().copied().filter(|&id| {
                !matches!(
                    self.lexical_grammar.nfa.states[id as usize],
                    NfaState::Advance { is_sep: true, .. }
                )
            }));
        }
        let (state_id, is_new) = self.add_state(nfa_states, eof_valid, guard_id);

        if is_new {
            debug!(
                "entry point state: {state_id}, tokens: {:?}",
                tokens
                    .iter()
                    .map(|token| {
                        if token.is_eof() {
                            "<EOF>"
                        } else {
                            debug_assert!(token.is_terminal());
                            self.str_pool
                                .resolve(self.lexical_grammar.variables[token.index as usize].name)
                        }
                    })
                    .collect::<Vec<_>>()
            );
        }

        while let Some(QueueEntry {
            state_id,
            nfa_states,
            eof_valid,
            guard_id,
        }) = self.state_queue.pop_front()
        {
            self.populate_state(state_id, nfa_states, eof_valid, guard_id);
        }
        state_id
    }

    fn add_state(
        &mut self,
        nfa_states: Vec<u32>,
        eof_valid: bool,
        guard_id: usize,
    ) -> (LexStateId, bool) {
        self.cursor.reset(nfa_states);
        match self.state_ids_by_nfa_state_set.entry((
            self.cursor.state_ids.clone(),
            eof_valid,
            guard_id,
        )) {
            Entry::Occupied(o) => (*o.get(), false),
            Entry::Vacant(v) => {
                let state_id = self.table.states.len() as u32;
                self.table.states.push(LexState::default());
                self.state_queue.push_back(QueueEntry {
                    state_id,
                    nfa_states: v.key().0.clone(),
                    eof_valid,
                    guard_id,
                });
                v.insert(state_id);
                (state_id, true)
            }
        }
    }

    fn populate_state(
        &mut self,
        state_id: LexStateId,
        nfa_states: Vec<u32>,
        eof_valid: bool,
        guard_id: usize,
    ) {
        self.cursor.force_reset(nfa_states);

        // The EOF state is represented as an empty list of NFA states.
        let immediate_complete = guard_id != 0
            && self
                .cursor
                .completions()
                .any(|(id, _)| self.immediate_keywords.contains(Symbol::terminal(id)));
        let mut completion = None;
        for (id, prec) in self.cursor.completions() {
            if immediate_complete
                && self
                    .word_token
                    .is_some_and(|word| word.index as usize == id)
            {
                continue;
            }
            if let Some((prev_id, prev_precedence)) = completion
                && TokenConflictMap::prefer_token(
                    self.lexical_grammar,
                    (prev_precedence, prev_id),
                    (prec, id),
                )
            {
                continue;
            }
            completion = Some((id, prec));
        }

        let retained_immediate_complete = guard_id != 0
            && completion.is_some_and(|(id, _)| {
                let token = Symbol::terminal(id);
                self.immediate_keywords.contains(token)
                    && !self.word_guards[guard_id].contains(token)
            });
        let (transitions, has_sep) = self.cursor.transitions_and_any_sep();

        // If EOF is a valid lookahead token, add a transition predicated on the null
        // character that leads to the empty set of NFA states.
        if eof_valid {
            let (next_state_id, _) = self.add_state(Vec::new(), false, guard_id);
            self.table.states[state_id as usize].eof_action = Some(AdvanceAction {
                state: next_state_id,
                in_main_token: true,
            });
        }

        for mut transition in transitions {
            let guarded_keyword_pending = guard_id != 0
                && self
                    .lexical_grammar
                    .variable_indices_for_nfa_states(&transition.states)
                    .any(|index| self.word_guards[guard_id].contains(Symbol::terminal(index)));
            if retained_immediate_complete && !guarded_keyword_pending {
                let word = self.word_token.unwrap();
                transition.states.retain(|&id| {
                    !self
                        .lexical_grammar
                        .variable_indices_for_nfa_states(&[id])
                        .any(|index| index == word.index as usize)
                });
                if transition.states.is_empty() {
                    continue;
                }
            }
            if let Some((completed_id, completed_precedence)) = completion
                && !(guard_id != 0
                    && (self.word_guards[guard_id].contains(Symbol::terminal(completed_id))
                        || (retained_immediate_complete && guarded_keyword_pending))
                    && self.word_token.is_some_and(|word| {
                        self.lexical_grammar
                            .variable_indices_for_nfa_states(&transition.states)
                            .any(|index| index == word.index as usize)
                    }))
                && !TokenConflictMap::prefer_transition(
                    self.lexical_grammar,
                    &transition,
                    completed_id,
                    completed_precedence,
                    has_sep,
                )
            {
                continue;
            }

            let (next_state_id, _) = self.add_state(
                transition.states,
                eof_valid && transition.is_separator,
                guard_id,
            );
            self.table.states[state_id as usize].advance_actions.push((
                transition.characters,
                AdvanceAction {
                    state: next_state_id,
                    in_main_token: !transition.is_separator,
                },
            ));
        }

        if let Some((complete_id, _)) = completion {
            self.table.states[state_id as usize].accept_action =
                Some(Symbol::terminal(complete_id));
        } else if self.cursor.state_ids.is_empty() {
            self.table.states[state_id as usize].accept_action = Some(Symbol::end());
        }
    }
}

fn check_token_conflicts(
    i: usize,
    set_without_terminal: &TokenSet,
    token_conflict_map: &TokenConflictMap,
    coincident_token_index: &CoincidentTokenIndex,
) -> bool {
    let wpr = token_conflict_map.row_words;
    let row_start = i * wpr;
    let set_bits = set_without_terminal.terminal_bits_words();

    // Does terminal i conflict with or match-prefix any terminal in the set?
    let conflict_row = &token_conflict_map.conflict_or_prefix_bits[row_start..row_start + wpr];
    for (&c, &s) in conflict_row.iter().zip(set_bits) {
        if c & s != 0 {
            return true;
        }
    }

    // Does terminal i overlap (in either direction) with any non-coincident terminal in the set?
    let overlap_row = &token_conflict_map.overlap_either_bits[row_start..row_start + wpr];
    let coincident_row = &coincident_token_index.row_bits[row_start..row_start + wpr];
    for ((&o, &s), &c) in overlap_row.iter().zip(set_bits).zip(coincident_row) {
        if o & s & !c != 0 {
            return true;
        }
    }

    false
}

fn merge_token_set(
    tokens: &mut TokenSet,
    other: &TokenSet,
    token_conflict_map: &TokenConflictMap,
    coincident_token_index: &CoincidentTokenIndex,
) -> bool {
    if tokens
        .terminals()
        .filter(|terminal| !other.contains_terminal(terminal.index as usize))
        .any(|terminal| {
            check_token_conflicts(
                terminal.index as usize,
                other,
                token_conflict_map,
                coincident_token_index,
            )
        })
    {
        return false;
    }

    if other
        .terminals()
        .filter(|terminal| !tokens.contains_terminal(terminal.index as usize))
        .any(|terminal| {
            check_token_conflicts(
                terminal.index as usize,
                tokens,
                token_conflict_map,
                coincident_token_index,
            )
        })
    {
        return false;
    }

    tokens.insert_all(other);
    true
}

fn coalesce_lex_transitions(table: &mut LexTable) -> bool {
    let mut changed = false;
    let mut action_indices = FxHashMap::<(LexStateId, bool), usize>::default();
    for state in &mut table.states {
        action_indices.clear();
        let mut actions: Vec<(CharacterSet, AdvanceAction)> = Vec::new();
        for (chars, action) in mem::take(&mut state.advance_actions) {
            let key = (action.state, action.in_main_token);
            if let Some(&index) = action_indices.get(&key) {
                let (existing_chars, _) = &mut actions[index];
                *existing_chars = mem::take(existing_chars).add(&chars);
                changed = true;
            } else {
                action_indices.insert(key, actions.len());
                actions.push((chars, action));
            }
        }
        actions.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        state.advance_actions = actions;
    }
    changed
}

fn minimize_lex_table(table: &mut LexTable) -> Vec<LexStateId> {
    // Initially group the states by their accept action and their
    // valid lookahead characters.
    let mut state_ids_by_signature = FxHashMap::default();
    for (i, state) in table.states.iter().enumerate() {
        let signature = (
            i == 0,
            state.accept_action,
            state.eof_action.is_some(),
            state
                .advance_actions
                .iter()
                .map(|(characters, action)| (characters.clone(), action.in_main_token))
                .collect::<Vec<_>>(),
        );
        state_ids_by_signature
            .entry(signature)
            .or_insert(Vec::new())
            .push(i as u32);
    }
    let mut state_ids_by_group_id = state_ids_by_signature
        .into_iter()
        .map(|e| e.1)
        .collect::<Vec<_>>();
    state_ids_by_group_id.sort();
    let error_group_index = state_ids_by_group_id
        .iter()
        .position(|g| g.contains(&0))
        .unwrap();
    state_ids_by_group_id.swap(error_group_index, 0);

    let mut group_ids_by_state_id = vec![0u32; table.states.len()];
    for (group_id, state_ids) in state_ids_by_group_id.iter().enumerate() {
        for state_id in state_ids {
            group_ids_by_state_id[*state_id as usize] = group_id as u32;
        }
    }

    while split_state_id_groups(
        &table.states,
        &mut state_ids_by_group_id,
        &mut group_ids_by_state_id,
        1,
        &mut LexStateSplit,
    ) {}

    let mut new_states = Vec::with_capacity(state_ids_by_group_id.len());
    for state_ids in &state_ids_by_group_id {
        let mut new_state = LexState::default();
        mem::swap(&mut new_state, &mut table.states[state_ids[0] as usize]);

        for (_, advance_action) in &mut new_state.advance_actions {
            advance_action.state = group_ids_by_state_id[advance_action.state as usize];
        }
        if let Some(eof_action) = &mut new_state.eof_action {
            eof_action.state = group_ids_by_state_id[eof_action.state as usize];
        }
        new_states.push(new_state);
    }

    table.states = new_states;
    group_ids_by_state_id
}

struct LexStateSplit;

impl SplitCriterion<LexState> for LexStateSplit {
    fn should_split(
        &mut self,
        left: &LexState,
        right: &LexState,
        group_ids_by_state_id: &[LexStateId],
    ) -> bool {
        lex_states_differ(left, right, group_ids_by_state_id)
    }

    /// The states of a group have the same advance actions apart from the states they lead
    /// to, so states that don't need to be split lead to the same groups. Every kept state
    /// leads to the same groups as the first one, and `state` was just found to as well.
    fn compatible_with_all(
        &mut self,
        _state: &LexState,
        _kept: &[u32],
        _group_ids_by_state_id: &[LexStateId],
    ) -> bool {
        true
    }
}

fn lex_states_differ(
    left: &LexState,
    right: &LexState,
    group_ids_by_state_id: &[LexStateId],
) -> bool {
    left.advance_actions
        .iter()
        .zip(right.advance_actions.iter())
        .any(|(left, right)| {
            group_ids_by_state_id[left.1.state as usize]
                != group_ids_by_state_id[right.1.state as usize]
        })
}

fn sort_states(table: &mut LexTable, parse_table: &mut ParseTable) {
    // Get a mapping of old state index -> new_state_index
    let mut old_ids_by_new_id = (0..table.states.len()).collect::<Vec<_>>();
    old_ids_by_new_id[1..].sort_by_key(|id| &table.states[*id]);

    // Get the inverse mapping
    let mut new_ids_by_old_id = vec![0u32; old_ids_by_new_id.len()];
    for (id, old_id) in old_ids_by_new_id.iter().enumerate() {
        new_ids_by_old_id[*old_id] = id as u32;
    }

    // Reorder the parse states and update their references to reflect
    // the new ordering.
    table.states = old_ids_by_new_id
        .iter()
        .map(|old_id| {
            let mut state = LexState::default();
            mem::swap(&mut state, &mut table.states[*old_id]);
            for (_, advance_action) in &mut state.advance_actions {
                advance_action.state = new_ids_by_old_id[advance_action.state as usize];
            }
            if let Some(eof_action) = &mut state.eof_action {
                eof_action.state = new_ids_by_old_id[eof_action.state as usize];
            }
            state
        })
        .collect();

    // Update the parse table's lex state references
    for state in &mut parse_table.states {
        state.lex_state_id = new_ids_by_old_id[state.lex_state_id as usize];
    }
}
