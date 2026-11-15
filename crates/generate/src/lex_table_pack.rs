use std::cmp::Reverse;

use rustc_hash::FxHashMap;

use super::{
    grammars::{LexicalGrammar, SyntaxGrammar},
    nfa::NfaState,
    render::{RenderError, RenderResult},
    rules::Symbol,
    tables::{AdvanceAction, LexState, LexTable},
};

const CODEPOINT_LIMIT: u32 = 0x11_0000;
const DIRECT_KEYWORD_LOOKUP_FLAG: u16 = 0x8000;
const WIDE_BASE_FLAG: u16 = 0x4000;
const MAX_TAG_COUNT: u32 = 0x3fff;
const EXTENDED_FLAG: u16 = 0x8000;
const SIMPLE_STATE_LIMIT: u32 = 0x1fff;
const SIMPLE_SYMBOL_LIMIT: u16 = 0x3fff;

#[derive(Default)]
struct DedupPool {
    words: Vec<u16>,
    offsets: FxHashMap<Vec<u16>, u16>,
}

impl DedupPool {
    fn intern(&mut self, record: Vec<u16>) -> RenderResult<u16> {
        if let Some(&encoded) = self.offsets.get(&record) {
            return Ok(encoded);
        }
        if self.words.len() >= usize::from(EXTENDED_FLAG) {
            return Err(RenderError::LexTable(
                "extended lexer table pool exceeds 32768 words".into(),
            ));
        }
        let encoded = EXTENDED_FLAG | (self.words.len() as u16);
        self.words.extend_from_slice(&record);
        self.offsets.insert(record, encoded);
        Ok(encoded)
    }
}

struct CharClasses {
    class_count: u16,
    ascii_class: [u16; 128],
    unicode_ends: Vec<u32>,
    unicode_classes: Vec<u16>,
    state_rows: Vec<Vec<(u16, u16)>>,
}

struct CombTable {
    template_count: u16,
    wide_base: bool,
    state_template: Vec<u16>,
    template_base: Vec<u32>,
    template_default: Vec<u16>,
    comb_check: Vec<u16>,
    comb_action: Vec<u16>,
}

type LexTemplate = (u16, Vec<(u16, u16)>);

pub fn can_use_direct_keyword_lookup(
    syntax_grammar: &SyntaxGrammar,
    lexical_grammar: &LexicalGrammar,
    table: &LexTable,
) -> bool {
    if syntax_grammar.word_token.is_none_or(|t| !t.is_terminal()) || table.states.is_empty() {
        return false;
    }
    let has_nul_or_eof = table.states.iter().any(|state| {
        state.eof_action.is_some()
            || state
                .advance_actions
                .iter()
                .any(|(set, action)| action.in_main_token && set.contains('\0'))
    });
    if has_nul_or_eof {
        return false;
    }
    !lexical_grammar.nfa.states.iter().any(|nfa_state| {
        let NfaState::Advance {
            chars,
            is_sep: true,
            ..
        } = nfa_state
        else {
            return false;
        };
        table.states[0]
            .advance_actions
            .iter()
            .any(|(kw_chars, action)| action.in_main_token && chars.does_intersect(kw_chars))
    })
}

pub fn pack_lex_table(
    table: &LexTable,
    symbol_ids: &FxHashMap<Symbol, u16>,
    allow_direct_keyword_lookup: bool,
) -> RenderResult<Vec<u16>> {
    let fallback_states = [LexState::default()];
    let states = if table.states.is_empty() {
        &fallback_states[..]
    } else {
        &table.states[..]
    };
    let state_count = u16::try_from(states.len())
        .map_err(|_| RenderError::LexTable("lexer state count exceeds 65535".into()))?;
    let tag_count = start_tag_count(states)?;
    let mut ext_transitions = DedupPool::default();
    let mut ext_accepts = DedupPool::default();
    let classes = build_char_classes(states, &mut ext_transitions)?;
    let state_accept = states
        .iter()
        .map(|state| encode_accept(state, symbol_ids, &mut ext_accepts))
        .collect::<RenderResult<Vec<_>>>()?;
    let comb = build_comb_table(&classes.state_rows, classes.class_count)?;
    let direct_kw = allow_direct_keyword_lookup
        && tag_count <= 1
        && ext_accepts.words.is_empty()
        && states.iter().all(|state| {
            state.eof_action.is_none()
                && state.advance_actions.iter().all(|(_, action)| {
                    action.start_tags.is_empty() || action.start_tags.as_slice() == [(0, None)]
                })
        });
    let tag_and_flags = tag_count
        | if comb.wide_base { WIDE_BASE_FLAG } else { 0 }
        | if direct_kw {
            DIRECT_KEYWORD_LOOKUP_FLAG
        } else {
            0
        };
    assemble_lex_table(
        (state_count, tag_and_flags),
        &classes,
        &state_accept,
        &comb,
        &ext_transitions.words,
        &ext_accepts.words,
    )
}

fn assemble_lex_table(
    (state_count, tag_and_flags): (u16, u16),
    classes: &CharClasses,
    state_accept: &[u16],
    comb: &CombTable,
    ext_transitions: &[u16],
    ext_accepts: &[u16],
) -> RenderResult<Vec<u16>> {
    let unicode_range_count = u16::try_from(classes.unicode_classes.len())
        .map_err(|_| RenderError::LexTable("unicode range count exceeds 65535".into()))?;
    let comb_len = u32::try_from(comb.comb_check.len())
        .map_err(|_| RenderError::LexTable("lexer comb table exceeds u32".into()))?;
    let state_stride = if comb.wide_base { 5 } else { 4 };
    let total_words = 141usize
        + classes.unicode_classes.len() * 3
        + usize::from(state_count) * state_stride
        + comb.comb_check.len() * 2
        + ext_transitions.len()
        + ext_accepts.len();
    let total_words_u32 = u32::try_from(total_words)
        .map_err(|_| RenderError::LexTable("lexer table exceeds u32 words".into()))?;
    let mut out = Vec::with_capacity(total_words);
    push_u32(&mut out, total_words_u32);
    out.extend([
        state_count,
        comb.template_count,
        classes.class_count,
        unicode_range_count,
        tag_and_flags,
    ]);
    push_u32(&mut out, comb_len);
    push_u32(&mut out, ext_transitions.len() as u32);
    push_u32(&mut out, ext_accepts.len() as u32);
    out.extend_from_slice(&classes.ascii_class);
    for &end in &classes.unicode_ends {
        push_u32(&mut out, end);
    }
    out.extend_from_slice(&classes.unicode_classes);
    push_state_meta(&mut out, comb, state_accept);
    for (&chk, &act) in comb.comb_check.iter().zip(&comb.comb_action) {
        out.extend([chk, act]);
    }
    out.extend_from_slice(ext_transitions);
    out.extend_from_slice(ext_accepts);
    debug_assert_eq!(out.len(), total_words);
    Ok(out)
}

fn push_state_meta(out: &mut Vec<u16>, comb: &CombTable, state_accept: &[u16]) {
    for (&tmpl, &acc) in comb.state_template.iter().zip(state_accept) {
        let base = comb.template_base[usize::from(tmpl)];
        let def = comb.template_default[usize::from(tmpl)];
        if comb.wide_base {
            push_u32(out, base);
        } else {
            out.push(base as u16);
        }
        out.extend([tmpl, def, acc]);
    }
}

fn push_u32(out: &mut Vec<u16>, value: u32) {
    out.push((value & 0xffff) as u16);
    out.push((value >> 16) as u16);
}

fn start_tag_count(states: &[LexState]) -> RenderResult<u16> {
    let max_tag = states
        .iter()
        .flat_map(|state| {
            state
                .start_tag
                .into_iter()
                .chain(state.fallback_accept.and_then(|(_, _, tag)| tag))
                .chain(
                    state
                        .advance_actions
                        .iter()
                        .map(|(_, action)| action)
                        .chain(state.eof_action.iter())
                        .flat_map(|action| {
                            action
                                .start_tags
                                .iter()
                                .flat_map(|&(target, source)| [Some(target), source])
                                .flatten()
                        }),
                )
        })
        .max();
    let count = max_tag.map_or(0, |tag| tag + 1);
    if count > MAX_TAG_COUNT {
        return Err(RenderError::LexTable(
            "lexer start tag count exceeds 16383".into(),
        ));
    }
    Ok(count as u16)
}

fn encode_action(action: &AdvanceAction, pool: &mut DedupPool) -> RenderResult<u16> {
    let next_state = u16::try_from(action.state)
        .map_err(|_| RenderError::LexTable("lexer target state exceeds 65535".into()))?;
    let skip = u16::from(!action.in_main_token);
    let is_simple_tags =
        action.start_tags.is_empty() || action.start_tags.as_slice() == [(0, None)];
    if is_simple_tags && action.state < SIMPLE_STATE_LIMIT {
        let tag0 = u16::from(!action.start_tags.is_empty());
        return Ok((skip << 14) | (tag0 << 13) | (next_state + 1));
    }
    if action.start_tags.len() >= usize::from(EXTENDED_FLAG) {
        return Err(RenderError::LexTable(
            "too many start_tags on transition".into(),
        ));
    }
    let mut record = Vec::with_capacity(2 + action.start_tags.len() * 2);
    record.push(next_state);
    record.push((skip << 15) | (action.start_tags.len() as u16));
    for &(target, source) in &action.start_tags {
        record.push(target as u16);
        record.push(source.map_or(u16::MAX, |tag| tag as u16));
    }
    pool.intern(record)
}

fn encode_accept(
    state: &LexState,
    symbol_ids: &FxHashMap<Symbol, u16>,
    pool: &mut DedupPool,
) -> RenderResult<u16> {
    let is_simple = state.fallback_accept.is_none()
        && !state.restore_fallback
        && state.guarded_precedence.is_none()
        && matches!(state.start_tag, None | Some(0));
    if is_simple {
        let Some(symbol) = state.accept_action else {
            return Ok(0);
        };
        let sym_id = symbol_ids[&symbol];
        if sym_id < SIMPLE_SYMBOL_LIMIT {
            let tag0 = u16::from(state.start_tag == Some(0));
            return Ok((tag0 << 14) | (sym_id + 1));
        }
    }
    pool.intern(build_ext_accept(state, symbol_ids))
}

fn build_ext_accept(state: &LexState, symbol_ids: &FxHashMap<Symbol, u16>) -> Vec<u16> {
    let has_accept = !state.restore_fallback && state.accept_action.is_some();
    let flags = u16::from(state.fallback_accept.is_some())
        | (u16::from(state.restore_fallback) << 1)
        | (u16::from(has_accept) << 2)
        | (u16::from(has_accept && state.guarded_precedence.is_some()) << 3)
        | (u16::from(has_accept && state.start_tag.is_some()) << 4);
    let mut record = vec![flags];
    if let Some((sym, prec, tag)) = state.fallback_accept {
        record.push(symbol_ids[&sym]);
        push_u32(&mut record, prec as u32);
        record.push(tag.map_or(u16::MAX, |idx| idx as u16));
    }
    if let Some(sym) = state.accept_action.filter(|_| !state.restore_fallback) {
        record.push(symbol_ids[&sym]);
        if let Some(prec) = state.guarded_precedence {
            push_u32(&mut record, prec as u32);
        }
        if let Some(tag) = state.start_tag {
            record.push(tag as u16);
        }
    }
    record
}

fn build_char_classes(states: &[LexState], ext_trans: &mut DedupPool) -> RenderResult<CharClasses> {
    let boundaries = collect_boundaries(states);
    let mut state_rows = vec![Vec::new(); states.len()];
    let sigs = collect_interval_sigs(states, &boundaries, ext_trans, &mut state_rows)?;
    let mut sig_to_class = FxHashMap::<Vec<(u16, u16)>, u16>::default();
    let mut next_class = 2u16;
    let mut interval_classes = Vec::with_capacity(sigs.len());
    for sig in sigs {
        let cls = if let Some(&existing) = sig_to_class.get(&sig) {
            existing
        } else {
            let assigned = next_class;
            next_class = next_class.checked_add(1).ok_or_else(|| {
                RenderError::LexTable("character class count exceeds 65535".into())
            })?;
            for &(state_idx, act) in &sig {
                state_rows[usize::from(state_idx)].push((assigned, act));
            }
            sig_to_class.insert(sig, assigned);
            assigned
        };
        interval_classes.push(cls);
    }
    let (ascii_class, unicode_ends, unicode_classes) =
        fill_class_tables(&boundaries, &interval_classes);
    Ok(CharClasses {
        class_count: next_class,
        ascii_class,
        unicode_ends,
        unicode_classes,
        state_rows,
    })
}

fn collect_boundaries(states: &[LexState]) -> Vec<u32> {
    let mut boundaries = vec![0, 0x80, CODEPOINT_LIMIT];
    for state in states {
        for (chars, _) in &state.advance_actions {
            for range in chars.codepoint_ranges() {
                boundaries.push(range.start);
                boundaries.push(range.end);
            }
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
}

fn collect_interval_sigs(
    states: &[LexState],
    boundaries: &[u32],
    ext_trans: &mut DedupPool,
    state_rows: &mut [Vec<(u16, u16)>],
) -> RenderResult<Vec<Vec<(u16, u16)>>> {
    let mut sigs = vec![Vec::new(); boundaries.len() - 1];
    for (state_idx, state) in states.iter().enumerate() {
        if let Some(eof_action) = &state.eof_action {
            let act = encode_action(eof_action, ext_trans)?;
            state_rows[state_idx].push((0, act));
        }
        for (chars, action) in &state.advance_actions {
            let act = encode_action(action, ext_trans)?;
            if chars.contains(char::MAX) {
                state_rows[state_idx].push((1, act));
            }
            for range in chars.codepoint_ranges() {
                let start_ix = boundaries.binary_search(&range.start).unwrap();
                let end_ix = boundaries.binary_search(&range.end).unwrap();
                for sig in &mut sigs[start_ix..end_ix] {
                    sig.push((state_idx as u16, act));
                }
            }
        }
    }
    Ok(sigs)
}

fn fill_class_tables(
    boundaries: &[u32],
    interval_classes: &[u16],
) -> ([u16; 128], Vec<u32>, Vec<u16>) {
    let mut ascii_class = [0u16; 128];
    let mut unicode_ends = Vec::new();
    let mut unicode_classes = Vec::new();
    for (ix, &cls) in interval_classes.iter().enumerate() {
        let start = boundaries[ix];
        let end = boundaries[ix + 1];
        if end <= 0x80 {
            ascii_class[start as usize..end as usize].fill(cls);
        } else if unicode_classes.last() == Some(&cls) {
            *unicode_ends.last_mut().unwrap() = end - 1;
        } else {
            unicode_ends.push(end - 1);
            unicode_classes.push(cls);
        }
    }
    (ascii_class, unicode_ends, unicode_classes)
}

fn build_comb_table(state_rows: &[Vec<(u16, u16)>], class_count: u16) -> RenderResult<CombTable> {
    let (state_template, templates) = dedup_templates(state_rows, class_count)?;
    let template_count = templates.len() as u16;
    let template_default = templates.iter().map(|&(def, _)| def).collect::<Vec<_>>();
    let mut order = (0..templates.len())
        .filter(|&id| !templates[id].1.is_empty())
        .collect::<Vec<_>>();
    order.sort_unstable_by_key(|&id| (Reverse(templates[id].1.len()), id));

    let mut template_base = vec![0u32; templates.len()];
    let mut comb_check = vec![u16::MAX; usize::from(class_count)];
    let mut comb_action = vec![0u16; usize::from(class_count)];
    let mut first_free_slot = 0usize;
    let mut max_base = 0usize;
    for tmpl_id in order {
        let base = place_template(
            tmpl_id as u16,
            &templates[tmpl_id].1,
            class_count,
            &mut first_free_slot,
            &mut comb_check,
            &mut comb_action,
        )?;
        template_base[tmpl_id] = base as u32;
        max_base = max_base.max(base);
    }
    let min_len = max_base + usize::from(class_count);
    if comb_check.len() < min_len {
        comb_check.resize(min_len, u16::MAX);
        comb_action.resize(min_len, 0);
    }
    Ok(CombTable {
        template_count,
        wide_base: u16::try_from(max_base).is_err(),
        state_template,
        template_base,
        template_default,
        comb_check,
        comb_action,
    })
}

fn dedup_templates(
    state_rows: &[Vec<(u16, u16)>],
    class_count: u16,
) -> RenderResult<(Vec<u16>, Vec<LexTemplate>)> {
    let mut templates = vec![(0u16, Vec::new())];
    let mut map = FxHashMap::default();
    map.insert((0u16, Vec::new()), 0u16);
    let mut state_template = Vec::with_capacity(state_rows.len());
    for row in state_rows {
        let key = template_for_row(row, class_count);
        let id = if let Some(&existing) = map.get(&key) {
            existing
        } else {
            if templates.len() >= usize::from(u16::MAX) {
                return Err(RenderError::LexTable(
                    "lexer template count exceeds 65534".into(),
                ));
            }
            let assigned = templates.len() as u16;
            templates.push(key.clone());
            map.insert(key, assigned);
            assigned
        };
        state_template.push(id);
    }
    Ok((state_template, templates))
}

fn template_for_row(row: &[(u16, u16)], class_count: u16) -> (u16, Vec<(u16, u16)>) {
    let mut full = vec![0u16; usize::from(class_count)];
    for &(cls, act) in row {
        full[usize::from(cls)] = act;
    }
    let mut counts = FxHashMap::<u16, usize>::default();
    for &act in &full {
        *counts.entry(act).or_default() += 1;
    }
    let mut default_act = 0u16;
    let mut best_count = counts.get(&0).copied().unwrap_or(0);
    for &(_, act) in row {
        let count = counts[&act];
        if count > best_count {
            best_count = count;
            default_act = act;
        }
    }
    let exceptions = full
        .into_iter()
        .enumerate()
        .filter_map(|(cls, act)| (act != default_act).then_some((cls as u16, act)))
        .collect();
    (default_act, exceptions)
}

fn place_template(
    tmpl_id: u16,
    exceptions: &[(u16, u16)],
    class_count: u16,
    first_free_slot: &mut usize,
    comb_check: &mut Vec<u16>,
    comb_action: &mut Vec<u16>,
) -> RenderResult<usize> {
    while *first_free_slot < comb_check.len() && comb_check[*first_free_slot] != u16::MAX {
        *first_free_slot += 1;
    }
    let first_cls = usize::from(exceptions[0].0);
    let mut base = first_free_slot.saturating_sub(first_cls);
    loop {
        let needed = base + usize::from(class_count);
        if comb_check.len() < needed {
            comb_check.resize(needed, u16::MAX);
            comb_action.resize(needed, 0);
        }
        if exceptions
            .iter()
            .all(|&(cls, _)| comb_check[base + usize::from(cls)] == u16::MAX)
        {
            for &(cls, act) in exceptions {
                let slot = base + usize::from(cls);
                comb_check[slot] = tmpl_id;
                comb_action[slot] = act;
            }
            return u32::try_from(base)
                .map(|_| base)
                .map_err(|_| RenderError::LexTable("lexer comb base exceeds u32".into()));
        }
        base += 1;
    }
}
