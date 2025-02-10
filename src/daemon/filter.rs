use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use chrono::Local;
use regex::Regex;
use tokio::sync::Semaphore;

use crate::{
    concepts::{Filter, Match, Pattern, Time},
    protocol::{Order, PatternStatus},
};

use super::{action::ActionManager, SledDbExt, SledTreeExt};

pub struct FilterManager {
    filter: &'static Filter,
    action_managers: Vec<ActionManager>,
    /// sled::Tree quivalent to BTreeMap<Match, BTreeSet<Time>>,
    matches: sled::Tree,
    /// sled::Tree quivalent to BTreeMap<Time, Match>
    ordered_times: sled::Tree,
}

#[allow(clippy::unwrap_used)]
impl FilterManager {
    pub fn new(
        filter: &'static Filter,
        exec_limit: Option<Arc<Semaphore>>,
        db: &sled::Db,
    ) -> Result<Self, sled::Error> {
        let manager = Self {
            filter,
            action_managers: filter
                .actions()
                .values()
                .map(|action| ActionManager::new(action, exec_limit.clone(), db))
                .collect::<Result<Vec<_>, _>>()?,
            matches: db.open_filter_matches_tree(filter)?,
            ordered_times: db.open_filter_ordered_times_tree(filter)?,
        };
        let now = Local::now();
        manager.clear_past_times(now);
        Ok(manager)
    }

    pub async fn handle_line(&self, line: &str) {
        if let Some(match_) = self.filter.get_match(line) {
            let now = Local::now();
            self.handle_match(match_, now).await;
        }
    }

    pub async fn handle_match(&self, m: Match, t: Time) {
        self.clear_past_times(t);

        let exec = match self.filter.retry() {
            None => true,
            Some(retry) => {
                self.add_match(&m, t);
                // Number of stored times for this match >= configured retry for this filter
                self.get_times(&m) >= retry as usize
            }
        };

        if exec {
            self.remove_match(&m);
            for manager in &self.action_managers {
                manager.handle_exec(m.clone(), t);
            }
        }
    }

    pub fn quit(&self) {
        self.action_managers
            .iter()
            .for_each(|manager| manager.quit());
    }

    pub async fn handle_order(
        &self,
        patterns: &BTreeMap<Arc<Pattern>, Regex>,
        order: Order,
    ) -> BTreeMap<String, PatternStatus> {
        let is_match = |match_: &Match| {
            match_
                .iter()
                .zip(self.filter.patterns())
                .filter_map(|(a_match, pattern)| {
                    patterns.get(pattern.as_ref()).map(|regex| (a_match, regex))
                })
                .all(|(a_match, regex)| regex.is_match(a_match))
        };

        let cs: BTreeMap<_, _> = self.matches
            .iter_ext::<Match, BTreeSet<Time>>()
            // match filtering
            .filter(|(match_, _)| is_match(match_))
            .map(|(match_, times)| {
                if let Order::Flush = order {
                    self.remove_match(&match_);
                }
                (
                    match_,
                    PatternStatus {
                        matches: times.len(),
                        ..Default::default()
                    },
                )
            })
            .collect();

        let cs = self.action_managers.iter().fold(cs, |mut acc, manager| {
            for (match_, times) in manager.handle_order(order, is_match) {
                let pattern_status = acc.entry(match_).or_default();
                pattern_status
                    .actions
                    .insert(manager.action().name().into(), times);
            }
            acc
        });

        cs.into_iter().map(|(k, v)| (k.join(" "), v)).collect()
    }

    fn add_match(&self, m: &Match, t: Time) {
        // FIXME do this in a transaction
        self.matches
            .fetch_and_update_ext::<Match, BTreeSet<Time>, _>(m, |set| {
                let mut set = set.unwrap_or_default();
                set.insert(t);
                Some(set)
            })
            .unwrap();
        self.ordered_times.insert_ext::<Time, Match>(&t, m).unwrap();
    }

    fn remove_match(&self, m: &Match) {
        // FIXME do this in a transaction
        if let Some(times) = self.matches.remove_ext::<Match, BTreeSet<Time>>(m) {
            for t in times {
                self.ordered_times.remove_ext::<Time, Match>(&t);
            }
        }
    }

    fn clear_past_times(&self, now: Time) {
        let retry_duration = self.filter.retry_duration().unwrap_or_default();
        while self
            .ordered_times
            .first_ext::<Time, Match>()
            .unwrap()
            .is_some_and(|(t, _)| t + retry_duration < now)
        {
            // FIXME do this in a transaction
            #[allow(clippy::unwrap_used)] // second unwrap: we just checked in the condition that first is_some
            let (t, m) = self
                .ordered_times
                .pop_min_ext::<Time, Match>()
                .unwrap()
                .unwrap();
            self.matches
                .fetch_and_update_ext::<Match, BTreeSet<Time>, _>(&m, |set| {
                    let mut set = set.unwrap();
                    set.remove(&t);
                    Some(set)
                })
                .unwrap();
        }
    }

    fn get_times(&self, m: &Match) -> usize {
        self.matches
            .get_ext::<Match, BTreeSet<Time>>(m)
            .unwrap()
            .map(|v| v.len())
            .unwrap_or(0)
    }
}
