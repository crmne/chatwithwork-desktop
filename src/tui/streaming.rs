//! Shared answer assembly for desktop and terminal clients. Chunk indexes
//! come from RubyLLM through the server and daemon unchanged. They can be
//! sparse: thinking and tool chunks use indexes without contributing text.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Deref;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Answers {
    text: BTreeMap<u64, String>,
    chunks: BTreeMap<u64, BTreeMap<u64, String>>,
    settled: BTreeSet<u64>,
}

impl Answers {
    pub fn push(&mut self, message: u64, seq: Option<u64>, text: String) {
        if self.settled.contains(&message) {
            return;
        }
        if let Some(seq) = seq {
            let chunks = self.chunks.entry(message).or_default();
            if chunks.contains_key(&seq) {
                return;
            }
            let append = chunks.last_key_value().is_some_and(|(last, _)| seq > *last);
            if append {
                self.text.entry(message).or_default().push_str(&text);
                chunks.insert(seq, text);
            } else {
                chunks.insert(seq, text);
                self.text
                    .insert(message, chunks.values().cloned().collect());
            }
        } else if !self.chunks.contains_key(&message) {
            // Compatibility with servers predating sequence numbers.
            self.text.entry(message).or_default().push_str(&text);
        }
    }

    /// The persisted transcript is authoritative. Late chunks must not
    /// recreate a streaming answer or make a finished chat look active.
    pub fn settle(&mut self, message: u64) {
        self.text.remove(&message);
        self.chunks.remove(&message);
        self.settled.insert(message);
    }

    pub fn finish(&mut self) {
        self.settled.extend(self.text.keys());
        self.text.clear();
        self.chunks.clear();
    }

    /// Navigating to another chat discards this chat's complete state.
    pub fn clear(&mut self) {
        self.text.clear();
        self.chunks.clear();
        self.settled.clear();
    }
}

impl Deref for Answers {
    type Target = BTreeMap<u64, String>;

    fn deref(&self) -> &Self::Target {
        &self.text
    }
}

impl<'a> IntoIterator for &'a Answers {
    type Item = (&'a u64, &'a String);
    type IntoIter = std::collections::btree_map::Iter<'a, u64, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.text.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::Answers;

    #[test]
    fn out_of_order_and_replayed_chunks_form_one_answer() {
        let mut answers = Answers::default();
        answers.push(9, Some(8), "world".into());
        answers.push(9, Some(3), "Hello ".into());
        answers.push(9, Some(8), "world".into());
        answers.push(9, Some(11), "!".into());
        assert_eq!(answers[&9], "Hello world!");
        answers.push(10, Some(3), "Another answer".into());
        assert_eq!(answers[&10], "Another answer");
        assert_eq!(answers[&9], "Hello world!");
    }

    #[test]
    fn transcript_reconciliation_ignores_late_chunks_and_navigation_resets_it() {
        let mut answers = Answers::default();
        answers.push(9, Some(1), "Saved".into());
        answers.settle(9);
        answers.push(9, Some(2), " late".into());
        assert!(answers.is_empty());
        answers.push(10, Some(1), "Finished".into());
        answers.finish();
        answers.push(10, Some(2), " late".into());
        assert!(answers.is_empty());
        answers.clear();
        answers.push(9, Some(1), "Reopened".into());
        assert_eq!(answers[&9], "Reopened");
    }

    #[test]
    fn old_servers_append_but_unindexed_replays_do_not_corrupt_an_indexed_answer() {
        let mut answers = Answers::default();
        answers.push(9, None, "Old ".into());
        answers.push(9, None, "server".into());
        assert_eq!(answers[&9], "Old server");
        answers.push(9, Some(4), "Indexed".into());
        answers.push(9, None, " replay".into());
        assert_eq!(answers[&9], "Indexed");
    }
}
