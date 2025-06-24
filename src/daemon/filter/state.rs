use std::collections::{BTreeMap, BTreeSet};

use crate::{
    concepts::{Filter, Match, MatchTime, Time},
    treedb::{
        helpers::{to_match, to_matchtime, to_time, to_u64},
        Database, Tree,
    },
};

pub fn filter_ordered_times_db_name(filter: &Filter) -> String {
    format!(
        "filter_ordered_times_{}.{}",
        filter.stream_name(),
        filter.name()
    )
}

pub fn filter_triggers_db_name(filter: &Filter) -> String {
    format!("filter_triggers_{}.{}", filter.stream_name(), filter.name())
}

/// Internal state of a [`FilterManager`].
/// Holds all data on current matches and triggers.
pub struct State {
    /// the Filter managed
    filter: &'static Filter,
    /// Has the filter at least an action with an after directive?
    has_after: bool,
    /// Saves all the current Matches for this Filter
    /// Has duplicate values for a key
    /// Not persisted
    pub matches: BTreeMap<Match, BTreeSet<Time>>,
    /// Alternative view of the current Matches for O(1) cleaning of old Matches
    /// without added async Tasks to remove them
    /// Persisted
    pub ordered_times: Tree<Time, Match>,
    /// Saves all the current Triggers for this Filter
    /// Persisted
    pub triggers: Tree<MatchTime, u64>,
}

impl State {
    pub fn new(
        filter: &'static Filter,
        has_after: bool,
        db: &mut Database,
        now: Time,
    ) -> Result<Self, String> {
        let mut this = Self {
            filter,
            has_after,
            matches: BTreeMap::new(),
            ordered_times: db.open_tree(
                filter_ordered_times_db_name(filter),
                filter.retry_duration().unwrap_or_default(),
                |(key, value)| Ok((to_time(&key)?, to_match(&value)?)),
            )?,
            triggers: db.open_tree(
                filter_triggers_db_name(filter),
                filter.retry_duration().unwrap_or_default(),
                |(key, value)| Ok((to_matchtime(&key)?, to_u64(&value)?)),
            )?,
        };
        this.clear_past_matches(now);
        this.load_matches_from_ordered_times();
        Ok(this)
    }

    pub fn add_match(&mut self, m: Match, t: Time) {
        let set = self.matches.entry(m.clone()).or_default();
        set.insert(t);
        self.ordered_times.insert(t, m);
    }

    pub fn add_trigger(&mut self, m: Match, t: Time) {
        // We record triggered filters only when there is an action with an `after` directive
        if self.has_after {
            // Add the (Match, Time) to the triggers map
            self.triggers
                .insert(MatchTime { m, t }, self.filter.actions().len() as u64);
        }
    }

    // Completely remove a Match from the matches
    pub fn remove_match(&mut self, m: &Match) {
        if let Some(set) = self.matches.get(m) {
            for t in set {
                self.ordered_times.remove(t);
            }
            self.matches.remove(m);
        }
    }

    /// Completely remove a Match from the triggers
    pub fn remove_trigger(&mut self, m: &Match, t: &Time) {
        self.triggers.remove(&MatchTime {
            m: m.clone(),
            t: *t,
        });
    }

    /// Returns whether we should still execute an action for this (Match, Time) trigger
    pub fn decrement_trigger(&mut self, m: &Match, t: Time) -> bool {
        // We record triggered filters only when there is an action with an `after` directive
        if self.has_after {
            let mut exec_needed = false;
            let mt = MatchTime { m: m.clone(), t };
            let count = self.triggers.get(&mt);
            if let Some(count) = count {
                exec_needed = true;
                if *count <= 1 {
                    self.triggers.remove(&mt);
                } else {
                    self.triggers.insert(mt, count - 1);
                }
            }
            exec_needed
        } else {
            true
        }
    }

    pub fn clear_past_matches(&mut self, now: Time) {
        let retry_duration = self.filter.retry_duration().unwrap_or_default();
        while self
            .ordered_times
            .first_key_value()
            .is_some_and(|(t, _)| *t + retry_duration < now)
        {
            #[allow(clippy::unwrap_used)]
            // unwrap: we just checked in the condition that first is_some
            let (t, m) = {
                let (t, m) = self.ordered_times.first_key_value().unwrap();
                (*t, m.clone())
            };
            self.ordered_times.remove(&t);
            if let Some(set) = self.matches.get(&m) {
                let mut set = set.clone();
                set.remove(&t);
                if set.is_empty() {
                    self.matches.remove(&m);
                } else {
                    self.matches.insert(m, set);
                }
            }
        }
    }

    pub fn get_times(&self, m: &Match) -> usize {
        match self.matches.get(m) {
            Some(vec) => vec.len(),
            None => 0,
        }
    }

    fn load_matches_from_ordered_times(&mut self) {
        for (t, m) in self.ordered_times.iter() {
            let set = self.matches.entry(m.clone()).or_default();
            set.insert(*t);
        }
    }
}
