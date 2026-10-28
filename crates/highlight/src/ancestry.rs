use super::{Error, metadata};
use std::{
    collections::{BTreeSet, HashMap},
    ops::Range,
    sync::atomic::AtomicUsize,
};
use tree_sitter::Node;

struct Entry {
    parent: usize,
    depth: usize,
    start: usize,
    end: usize,
    range: Range<usize>,
}

struct Frame {
    id: usize,
    start: usize,
    range: Range<usize>,
    needed: bool,
}

pub struct Ancestry {
    entries: HashMap<usize, Entry>,
}

impl Ancestry {
    pub(super) fn new(
        root: Node,
        targets: &HashMap<usize, Range<usize>>,
        cancellation: Option<&AtomicUsize>,
    ) -> Result<Self, Error> {
        let mut remaining: BTreeSet<_> = targets
            .iter()
            .map(|(id, range)| (range.start, range.end, *id))
            .collect();
        let mut cursor = root.walk();
        let mut frames: Vec<Frame> = Vec::new();
        let mut entries = HashMap::new();
        let mut position = 0;
        loop {
            if metadata::is_cancelled(cancellation) {
                return Err(Error::Cancelled);
            }
            let node = cursor.node();
            let range = node.byte_range();
            frames.push(Frame {
                id: node.id(),
                start: position,
                range: range.clone(),
                needed: targets
                    .get(&node.id())
                    .is_some_and(|range| remaining.remove(&(range.start, range.end, node.id()))),
            });
            position += 1;
            if contains_remaining(&range, &remaining) && cursor.goto_first_child() {
                continue;
            }
            loop {
                if metadata::is_cancelled(cancellation) {
                    return Err(Error::Cancelled);
                }
                let frame = frames.pop().unwrap();
                if frame.needed {
                    let parent = frames.last_mut().map_or(frame.id, |parent| {
                        parent.needed = true;
                        parent.id
                    });
                    entries.insert(
                        frame.id,
                        Entry {
                            parent,
                            depth: frames.len(),
                            start: frame.start,
                            end: position,
                            range: frame.range,
                        },
                    );
                }
                if !remaining.is_empty() && cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    return Ok(Self { entries });
                }
            }
        }
    }

    pub(super) fn contains(&self, ancestor: usize, descendant: usize) -> bool {
        self.entries
            .get(&ancestor)
            .zip(self.entries.get(&descendant))
            .is_some_and(|(ancestor, descendant)| {
                (ancestor.start..ancestor.end).contains(&descendant.start)
            })
    }

    pub(super) fn depth(&self, node: usize) -> usize {
        self.entries[&node].depth
    }

    pub(super) fn common_range(&self, mut left: usize, mut right: usize) -> &Range<usize> {
        while self.entries[&left].depth > self.entries[&right].depth {
            left = self.entries[&left].parent;
        }
        while self.entries[&right].depth > self.entries[&left].depth {
            right = self.entries[&right].parent;
        }
        while left != right {
            left = self.entries[&left].parent;
            right = self.entries[&right].parent;
        }
        &self.entries[&left].range
    }
}

fn contains_remaining(range: &Range<usize>, remaining: &BTreeSet<(usize, usize, usize)>) -> bool {
    remaining
        .range((range.start, 0, 0)..=(range.end, usize::MAX, usize::MAX))
        .any(|(_, end, _)| *end <= range.end)
}
