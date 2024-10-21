use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use chrono::Local;
use regex::Regex;
use tokio::sync::{mpsc, Semaphore};

use crate::concepts::{Filter, LogEntry, Match, Order, Pattern, PatternStatus, Time, MFT};

use super::{action::ActionManager, database::DatabaseManagerInput};

pub struct FilterManager {
    filter: &'static Filter,
    log_tx: mpsc::Sender<DatabaseManagerInput>,
    action_managers: Vec<ActionManager>,
    matches: BTreeMap<Match, BTreeSet<Time>>,
    ordered_times: BTreeMap<Time, Match>,
}

impl FilterManager {
    pub fn new(
        filter: &'static Filter,
        matches: BTreeMap<Match, BTreeSet<Time>>,
        exec_limit: Option<Arc<Semaphore>>,
        log_tx: mpsc::Sender<DatabaseManagerInput>,
    ) -> Self {
        Self {
            filter,
            log_tx,
            action_managers: filter
                .actions()
                .values()
                .map(|action| ActionManager::new(action, BTreeMap::default(), exec_limit.clone()))
                .collect(),
            matches: matches.clone(),
            ordered_times: matches
                .into_iter()
                .flat_map(|(m, times)| times.into_iter().map(move |time| (time, m.clone())))
                .collect(),
        }
    }

    pub async fn handle_db_entries(mut self, mut match_rx: mpsc::Receiver<MFT>) -> Self {
        while let Some(mft) = match_rx.recv().await {
            self.handle_match(mft.m, mft.t, false).await;
        }
        self
    }

    pub async fn handle_line(&mut self, line: &str) {
        if let Some(match_) = self.filter.get_match(line) {
            let now = Local::now();
            self.handle_match(match_, now, true).await;
        }
    }

    pub async fn handle_match(&mut self, m: Match, t: Time, send_log: bool) {
        self.clear_past_times();

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

        if send_log {
            #[allow(clippy::unwrap_used)] // propagating panics is ok
            self.log_tx
                .send(DatabaseManagerInput::Log(LogEntry {
                    exec,
                    m,
                    f: self.filter,
                    t,
                }))
                .await
                .unwrap();
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

        let matches = self.matches.clone();
        let cs: BTreeMap<_, _> = matches
            .into_iter()
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
                    .insert(manager.action().to_string(), times);
            }
            acc
        });

        let now = Local::now();
        for match_ in cs.keys() {
            #[allow(clippy::unwrap_used)] // propagating panics is ok
            self.log_tx
                .send(DatabaseManagerInput::Flush(LogEntry {
                    exec: false,
                    m: match_.to_vec(),
                    f: self.filter,
                    t: now,
                }))
                .await
                .unwrap()
        }

        cs.into_iter().map(|(k, v)| (k.join(" "), v)).collect()
    }

    fn add_match(&mut self, m: &Match, t: Time) {
        self.matches.entry(m.clone()).or_default().insert(t);
        self.ordered_times.insert(t, m.clone());
    }

    fn remove_match(&mut self, m: &Match) {
        if let Some(times) = self.matches.remove(m) {
            for t in times {
                self.ordered_times.remove(&t);
            }
        }
    }

    fn clear_past_times(&mut self) {
        let now = Local::now();
        let retry_duration = self.filter.retry_duration().unwrap_or_default();
        while self
            .ordered_times
            .first_key_value()
            .is_some_and(|(k, _)| *k + retry_duration < now)
        {
            #[allow(clippy::unwrap_used)] // we just checked in the condition that first is_some
            let (_, m) = self.ordered_times.pop_first().unwrap();
            self.matches.remove(&m);
        }
    }

    fn get_times(&self, m: &Match) -> usize {
        self.matches.get(m).map(|v| v.len()).unwrap_or(0)
    }
}
