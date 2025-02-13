use std::{
    collections::{BTreeMap, BTreeSet},
    process::Stdio,
    sync::Arc,
};

use chrono::Local;
use regex::Regex;
use tokio::sync::Semaphore;
use tracing::{error, info};

use crate::{
    concepts::{Action, Filter, Match, Pattern, Time},
    protocol::{Order, PatternStatus},
};

use super::{shutdown::ShutdownToken, SledDbExt, SledTreeExt};

#[derive(Clone)]
pub struct FilterManager {
    filter: &'static Filter,
    exec_limit: Option<Arc<Semaphore>>,
    shutdown: ShutdownToken,
    /// sled::Tree equivalent to BTreeMap<Match, BTreeSet<Time>>
    matches: sled::Tree,
    /// sled::Tree equivalent to BTreeMap<Time, Match>
    ordered_times: sled::Tree,
    /// sled::Tree equivalent to BTreeMap<Match, BTreeSet<Time>>
    triggers: sled::Tree,
}

#[allow(clippy::unwrap_used)]
impl FilterManager {
    pub fn new(
        filter: &'static Filter,
        exec_limit: Option<Arc<Semaphore>>,
        shutdown: ShutdownToken,
        db: &sled::Db,
    ) -> Result<Self, sled::Error> {
        let manager = Self {
            filter,
            exec_limit,
            shutdown,
            matches: db.open_filter_matches_tree(filter)?,
            ordered_times: db.open_filter_ordered_times_tree(filter)?,
            triggers: db.open_filter_triggers_tree(filter)?,
        };
        let now = Local::now();
        manager.clear_past_matches(now);
        manager.clear_past_triggers_and_schedule_future_actions(now);
        Ok(manager)
    }

    pub fn handle_line(&self, line: &str) {
        if let Some(match_) = self.filter.get_match(line) {
            self.handle_match(match_);
        }
    }

    fn handle_match(&self, m: Match) {
        let now = Local::now();
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
            self.add_exec(&m, &now);
            self.schedule_exec(m.clone(), now);
        }
    }

    pub fn handle_order(
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

        let mut cs: BTreeMap<_, _> = self
            .matches
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

        let now = Local::now();
        for (match_, times) in self
            .triggers
            .iter_ext::<Match, BTreeSet<Time>>()
            // match filtering
            .filter(|(match_, _)| is_match(match_))
        {
            // Remove the match from the triggers
            if let Order::Flush = order {
                self.remove_exec(&match_);
            }

            let pattern_status = cs.entry(match_.clone()).or_default();

            for action in self.filter.actions().values() {
                let mut action_times = Vec::new();
                for time in &times {
                    let action_time = *time + action.after_duration().unwrap_or_default();
                    if action_time > now {
                        action_times.push(action_time.to_rfc3339().chars().take(19).collect());
                        // Execute the action early
                        if let Order::Flush = order {
                            Self::exec_now(self.exec_limit.clone(), action, match_.clone());
                        }
                    }
                }
                if !action_times.is_empty() {
                    pattern_status
                        .actions
                        .insert(action.name().into(), action_times);
                }
            }
        }

        cs.into_iter().map(|(k, v)| (k.join(" "), v)).collect()
    }

    /// Schedule execution for a given Action and Match.
    /// We check first if the trigger is still here
    /// because pending actions can be flushed.
    fn schedule_exec(&self, m: Match, t: Time) {
        let now = Local::now();
        for action in self.filter.actions().values() {
            let exec_time = t + action.after_duration().unwrap_or_default();
            let exec_limit = self.exec_limit.clone();
            let m = m.clone();

            if exec_time < now {
                Self::exec_now(exec_limit, action, m);
            } else {
                let triggers = self.triggers.clone();
                let shutdown = self.shutdown.clone();
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
                        _ = shutdown.wait() => true,
                    };
                    if (!exiting || action.on_exit())
                        && triggers
                            .get_ext::<Match, BTreeSet<Time>>(&m)
                            .unwrap()
                            .is_some_and(|set| set.contains(&t))
                    {
                        Self::exec_now(exec_limit, action, m);
                    }
                });
            }
        }
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

    fn add_exec(&self, m: &Match, t: &Time) {
        let sleep_duration = self.filter.longuest_action_duration().to_std().unwrap();
        // We record triggered filters only when there is an action with an `after` directive
        if !sleep_duration.is_zero() {
            // Add the (Match, Time) to the triggers map
            self.triggers
                .fetch_and_update_ext::<Match, BTreeSet<Time>, _>(m, |set| {
                    let mut set = set.unwrap_or_default();
                    set.insert(*t);
                    Some(set)
                })
                .unwrap();

            // Remove the (Match, Time) of the triggers map
            // when its last action is planned
            let triggers = self.triggers.clone();
            let m = m.clone();
            let t = *t;
            tokio::spawn(async move {
                tokio::time::sleep(sleep_duration).await;
                triggers
                    .fetch_and_update_ext::<Match, BTreeSet<Time>, _>(&m, |set| {
                        let mut set = set.unwrap_or_default();
                        set.remove(&t);
                        Some(set)
                    })
                    .unwrap();
            });
        }
    }

    fn remove_match(&self, m: &Match) {
        // FIXME do this in a transaction
        if let Some(times) = self.matches.remove_ext::<Match, BTreeSet<Time>>(m) {
            for t in times {
                self.ordered_times.remove_ext::<Time, Match>(&t);
            }
        }
    }

    fn remove_exec(&self, m: &Match) {
        // FIXME do this in a transaction
        self.triggers.remove_ext::<Match, BTreeSet<Time>>(m);
    }

    fn clear_past_matches(&self, now: Time) {
        let retry_duration = self.filter.retry_duration().unwrap_or_default();
        while self
            .ordered_times
            .first_ext::<Time, Match>()
            .unwrap()
            .is_some_and(|(t, _)| t + retry_duration < now)
        {
            // FIXME do this in a transaction
            #[allow(clippy::unwrap_used)]
            // second unwrap: we just checked in the condition that first is_some
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

    fn clear_past_triggers_and_schedule_future_actions(&self, now: Time) {
        let longuest_action_duration = self.filter.longuest_action_duration();

        for (m, set) in self.triggers.iter_ext::<Match, BTreeSet<Time>>() {
            // Keep only times that are still relevant
            let new_set: BTreeSet<_> = set
                .into_iter()
                .filter(|t| *t + longuest_action_duration > now)
                .collect();

            if new_set.is_empty() {
                // No upcoming time, delete the entry from the Tree
                self.triggers.remove_ext::<Match, BTreeSet<Time>>(&m);
            } else {
                // Insert back the upcoming times
                let _ = self
                    .triggers
                    .insert_ext::<Match, BTreeSet<Time>>(&m, &new_set);

                // Schedule the upcoming times
                for t in new_set.into_iter() {
                    self.schedule_exec(m.clone(), t);
                }
            }
        }
    }

    fn exec_now(semaphore: Option<Arc<Semaphore>>, action: &'static Action, m: Match) {
        tokio::spawn(async move {
            // Wait for semaphore's permission, if it is Some
            let _permit = match semaphore {
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
