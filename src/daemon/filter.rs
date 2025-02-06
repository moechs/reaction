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

use super::{action::ActionManager, SledTreeExt};

pub struct FilterManager {
    filter: &'static Filter,
    action_managers: Vec<ActionManager>,
    // BTreeMap<Match, BTreeSet<Time>>
    matches: sled::Tree,
    // BTreeMap<Time, Match>
    ordered_times: sled::Tree,
}

#[allow(clippy::unwrap_used)]
impl FilterManager {
    pub fn new(
        filter: &'static Filter,
        exec_limit: Option<Arc<Semaphore>>,
        db: &sled::Db,
    ) -> Result<Self, sled::Error> {
        let mut manager = Self {
            filter,
            action_managers: filter
                .actions()
                .values()
                .map(|action| ActionManager::new(action, BTreeMap::default(), exec_limit.clone()))
                .collect(),
            matches: db.open_tree(
                format!("matches_{}.{}", filter.stream_name(), filter.name()).as_bytes(),
            )?,
            ordered_times: db.open_tree(
                format!("ordered_times_{}.{}", filter.stream_name(), filter.name()).as_bytes(),
            )?,
        };
        let now = Local::now();
        manager.clear_past_times(now);
        Ok(manager)
    }

    pub async fn handle_line(&mut self, line: &str) {
        if let Some(match_) = self.filter.get_match(line) {
            let now = Local::now();
            self.handle_match(match_, now).await;
        }
    }

    pub async fn handle_match(&mut self, m: Match, t: Time) {
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
            for manager in &mut self.action_managers {
                manager.handle_exec(m.clone(), t);
            }
        }
    }

    pub fn quit(&mut self) {
        self.action_managers
            .iter_mut()
            .for_each(|manager| manager.quit());
    }

    pub async fn handle_order(
        &mut self,
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

        let cs: BTreeMap<_, _> = self
            .matches
            .iter()
            .map(|elt| {
                let (k, v) = elt.unwrap();
                let k: Match = bincode::deserialize(&k).unwrap();
                let v: BTreeSet<Time> = bincode::deserialize(&v).unwrap();
                (k, v)
            })
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

    fn add_match(&mut self, m: &Match, t: Time) {
        self.matches
            .fetch_and_update_ext::<Match, BTreeSet<Time>, _>(m, |set| {
                let mut set = set.unwrap_or_default();
                set.insert(t);
                Some(set)
            })
            .unwrap();
        self.ordered_times.insert_ext::<Time, Match>(&t, m).unwrap();
    }

    fn remove_match(&mut self, m: &Match) {
        if let Some(times) = self.matches.remove_ext::<Match, BTreeSet<Time>>(m) {
            for t in times {
                self.ordered_times.remove_ext::<Time, Match>(&t);
            }
        }
    }

    fn clear_past_times(&mut self, now: Time) {
        let retry_duration = self.filter.retry_duration().unwrap_or_default();
        while self
            .ordered_times
            .first_ext::<Time, Match>()
            .unwrap()
            .is_some_and(|(k, _)| k + retry_duration < now)
        {
            #[allow(clippy::unwrap_used)] // we just checked in the condition that first is_some
            let (_, m) = self
                .ordered_times
                .pop_min_ext::<Time, Match>()
                .unwrap()
                .unwrap();
            self.matches.remove_ext::<Match, BTreeSet<Time>>(&m);
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
