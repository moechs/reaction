#[cfg(test)]
mod tests;

use std::{collections::BTreeMap, process::Stdio, sync::Arc};

use heed::{
    byteorder::LittleEndian,
    types::{SerdeBincode, U64},
    Database,
};
use regex::Regex;
use tokio::sync::Semaphore;
use tracing::{error, info};

use crate::{
    concepts::{Action, Filter, Match, Pattern, Time},
    protocol::{Order, PatternStatus},
};

use super::{shutdown::ShutdownToken, EnvExt as _, MatchTime};

#[derive(Clone)]
pub struct FilterManager {
    /// the Filter managed
    filter: &'static Filter,
    /// Has the filter at least an action with an after directive?
    has_after: bool,
    /// Permits to limit concurrency of actions execution
    exec_limit: Option<Arc<Semaphore>>,
    /// Permits to run pending actions on shutdown
    shutdown: ShutdownToken,
    /// LMDB environment, necessary to create transactions
    env: heed::Env,
    /// Saves all the current Matches for this Filter
    /// Has duplicate values for a key
    matches: Database<SerdeBincode<Match>, SerdeBincode<Time>>,
    /// Alternative view of the current Matches for O(1) cleaning of old Matches
    /// without added async Tasks to remove them
    ordered_times: Database<SerdeBincode<Time>, SerdeBincode<Match>>,
    /// Saves all the current Triggers for this Filter
    triggers: Database<SerdeBincode<MatchTime>, U64<LittleEndian>>,
}

#[allow(clippy::unwrap_used)]
impl FilterManager {
    pub fn new(
        filter: &'static Filter,
        exec_limit: Option<Arc<Semaphore>>,
        shutdown: ShutdownToken,
        env: &heed::Env,
        now: Time,
    ) -> Result<Self, heed::Error> {
        let manager = Self {
            filter,
            has_after: !filter.longuest_action_duration().is_zero(),
            exec_limit,
            shutdown,
            env: env.clone(),
            matches: env.open_filter_matches_db(filter)?,
            ordered_times: env.open_filter_ordered_times_db(filter)?,
            triggers: env.open_filter_triggers_db(filter)?,
        };
        manager.clear_past_matches(now);
        manager.clear_past_triggers_and_schedule_future_actions(now);
        Ok(manager)
    }

    pub fn handle_line(&self, line: &str, now: Time) -> bool {
        if let Some(match_) = self.filter.get_match(line) {
            self.handle_match(match_, now);
            true
        } else {
            false
        }
    }

    fn handle_match(&self, m: Match, now: Time) {
        self.clear_past_matches(now);

        let exec = match self.filter.retry() {
            None => true,
            Some(retry) => {
                self.add_match(&m, now);
                // Number of stored times for this match >= configured retry for this filter
                self.get_times(&m) >= retry as usize
            }
        };

        if exec {
            self.remove_match(&m);
            self.add_trigger(m.clone(), now);
            self.schedule_exec(m, now, now);
        }
    }

    pub fn handle_order(
        &self,
        patterns: &BTreeMap<Arc<Pattern>, Regex>,
        order: Order,
        now: Time,
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

        let rtxn = self.env.read_txn().unwrap();
        let mut cs = BTreeMap::new();
        {
            let mut last_key: Option<Match> = None;
            let mut n_times = 0;

            let mut insert_last_key = |last_key: Option<Match>, n_times: usize| {
                if let Some(last_key) = last_key {
                    if let Order::Flush = order {
                        self.remove_match(&last_key);
                    }
                    cs.insert(
                        last_key,
                        PatternStatus {
                            matches: n_times,
                            ..Default::default()
                        },
                    );
                }
            };

            for (k, _) in self
                .matches
                .iter(&rtxn)
                .unwrap()
                .map(|result| result.unwrap())
                // match filtering
                .filter(|(match_, _)| is_match(match_))
            {
                if last_key.clone().is_some_and(|last_key| last_key == k) {
                    n_times += 1;
                } else {
                    insert_last_key(last_key, n_times);
                    n_times = 0;
                    last_key = Some(k);
                }
            }
            insert_last_key(last_key, n_times);
        }

        // We prefer closing & reopening the read transaction
        // because it's recommended to avoid long-lived transactions
        rtxn.commit().unwrap();
        let rtxn = self.env.read_txn().unwrap();

        for (mt, _) in self
            .triggers
            .iter(&rtxn)
            .unwrap()
            .map(|result| result.unwrap())
            // match filtering
            .filter(|(match_, _)| is_match(&match_.m))
        {
            // Remove the match from the triggers
            if let Order::Flush = order {
                // delete specific (Match, Time) tuple
                self.remove_trigger(&mt.m, &mt.t);
            }

            let m = mt.m.clone();
            let pattern_status = cs.entry(m).or_default();

            for action in self.filter.actions().values() {
                let action_times = pattern_status
                    .actions
                    .entry(action.name().into())
                    .or_default();

                let action_time = mt.t + action.after_duration().unwrap_or_default();
                if action_time > now {
                    action_times.push(action_time.to_rfc3339().chars().take(19).collect());
                    // Execute the action early
                    if let Order::Flush = order {
                        self.exec_now(action, mt.m.clone());
                    }
                }
            }
        }
        rtxn.commit().unwrap();

        cs.into_iter().map(|(k, v)| (k.join(" "), v)).collect()
    }

    /// Schedule execution for a given Action and Match.
    /// We check first if the trigger is still here
    /// because pending actions can be flushed.
    fn schedule_exec(&self, m: Match, t: Time, now: Time) {
        for action in self.filter.actions().values() {
            let exec_time = t + action.after_duration().unwrap_or_default();
            let m = m.clone();

            if exec_time <= now {
                if self.decrement_trigger(&m, t) {
                    self.exec_now(action, m);
                }
            } else {
                let this = self.clone();
                tokio::spawn(async move {
                    let dur = (exec_time - now)
                        .to_std()
                        // Could cause an error if t + after < now
                        // In this case, 0 is fine
                        .unwrap_or_default();
                    // Wait either for end of sleep
                    // or reaction exiting
                    let exiting = tokio::select! {
                        _ = tokio::time::sleep(dur) => false,
                        _ = this.shutdown.wait() => true,
                    };
                    // Exec action if triggered hasn't been already flushed
                    if (!exiting || action.on_exit()) && this.decrement_trigger(&m, t) {
                        this.exec_now(action, m);
                    }
                });
            }
        }
    }

    fn add_match(&self, m: &Match, t: Time) {
        let mut wtxn = self.env.write_txn().unwrap();
        self.matches.put(&mut wtxn, m, &t).unwrap();
        self.ordered_times.put(&mut wtxn, &t, m).unwrap();
        wtxn.commit().unwrap();
    }

    fn add_trigger(&self, m: Match, t: Time) {
        // We record triggered filters only when there is an action with an `after` directive
        if self.has_after {
            // Add the (Match, Time) to the triggers map
            let mut wtxn = self.env.write_txn().unwrap();
            self.triggers
                .put(
                    &mut wtxn,
                    &MatchTime { m, t },
                    &(self.filter.actions().len() as u64),
                )
                .unwrap();
            wtxn.commit().unwrap();
        }
    }

    // Completely remove a Match from the matches
    fn remove_match(&self, m: &Match) {
        let rtxn = self.env.read_txn().unwrap();
        let mut wtxn = self.env.write_txn().unwrap();
        if let Some(iter) = self.matches.get_duplicates(&rtxn, m).unwrap() {
            for t in iter {
                let t = t.unwrap().1;
                self.ordered_times.delete(&mut wtxn, &t).unwrap();
            }
        }
        rtxn.commit().unwrap();
        self.matches.delete(&mut wtxn, m).unwrap();
        wtxn.commit().unwrap();
    }

    /// Completely remove a Match from the triggers
    fn remove_trigger(&self, m: &Match, t: &Time) {
        let mut wtxn = self.env.write_txn().unwrap();
        self.triggers
            .delete(
                &mut wtxn,
                &MatchTime {
                    m: m.clone(),
                    t: *t,
                },
            )
            .unwrap();
        wtxn.commit().unwrap();
    }

    /// Returns whether we should still execute an action for this (Match, Time) trigger
    fn decrement_trigger(&self, m: &Match, t: Time) -> bool {
        // We record triggered filters only when there is an action with an `after` directive
        if self.has_after {
            let mut exec_needed = false;
            let mut wtxn = self.env.write_txn().unwrap();
            let mt = MatchTime { m: m.clone(), t };
            let count = self.triggers.get(&wtxn, &mt).unwrap();
            if let Some(count) = count {
                exec_needed = true;
                if count <= 1 {
                    self.triggers.delete(&mut wtxn, &mt).unwrap();
                } else {
                    self.triggers.put(&mut wtxn, &mt, &(count - 1)).unwrap();
                }
                wtxn.commit().unwrap();
            }
            exec_needed
        } else {
            true
        }
    }

    fn clear_past_matches(&self, now: Time) {
        let retry_duration = self.filter.retry_duration().unwrap_or_default();
        let mut wtxn = self.env.write_txn().unwrap();
        while self
            .ordered_times
            .first(&wtxn)
            .unwrap()
            .is_some_and(|(t, _)| t + retry_duration < now)
        {
            #[allow(clippy::unwrap_used)]
            // second unwrap: we just checked in the condition that first is_some
            let (t, m) = self.ordered_times.first(&wtxn).unwrap().unwrap();
            self.ordered_times.delete(&mut wtxn, &t).unwrap();
            self.matches
                .delete_one_duplicate(&mut wtxn, &m, &t)
                .unwrap();
            wtxn.commit().unwrap();
            wtxn = self.env.write_txn().unwrap();
        }
    }

    fn get_times(&self, m: &Match) -> usize {
        let rtxn = self.env.read_txn().unwrap();
        let ret = self
            .matches
            .get_duplicates(&rtxn, m)
            .unwrap()
            .map(|iter| iter.count())
            .unwrap_or(0);
        rtxn.commit().unwrap();
        ret
    }

    fn clear_past_triggers_and_schedule_future_actions(&self, now: Time) {
        let longuest_action_duration = self.filter.longuest_action_duration();
        let number_of_actions = self.filter.actions().len();

        let rtxn = self.env.read_txn().unwrap();
        let mut wtxn = self.env.write_txn().unwrap();
        for (mt, remaining) in self.triggers.iter(&rtxn).unwrap().map(|elt| elt.unwrap()) {
            if remaining > 0 && mt.t + longuest_action_duration > now {
                // Insert back the upcoming times
                self.triggers
                    .put(&mut wtxn, &mt, &(number_of_actions as u64))
                    .unwrap();
                // Schedule the upcoming times
                self.schedule_exec(mt.m, mt.t, now);
            } else {
                self.triggers.delete(&mut wtxn, &mt).unwrap();
            }
        }
        rtxn.commit().unwrap();
        wtxn.commit().unwrap();
    }

    fn exec_now(&self, action: &'static Action, m: Match) {
        let exec_limit = self.exec_limit.clone();
        tokio::spawn(async move {
            // Wait for semaphore's permission, if it is Some
            let _permit = match exec_limit {
                #[allow(clippy::unwrap_used)] // We know the semaphore is not closed
                Some(semaphore) => Some(semaphore.acquire_owned().await.unwrap()),
                None => None,
            };

            // Construct command
            let mut command = action.exec(&m);

            info!("{}: run [{:?}]", &action, command.as_std());
            if let Err(err) = command
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .stdout(Stdio::piped())
                .status()
                .await
            {
                error!("{}: run [{:?}], code {}", &action, command.as_std(), err);
            }
        });
    }
}
