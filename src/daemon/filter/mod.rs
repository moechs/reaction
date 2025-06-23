#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, BTreeSet},
    process::Stdio,
    sync::{Arc, Mutex, MutexGuard},
};

use regex::Regex;
use tokio::sync::Semaphore;
use tracing::{error, info};

use crate::{
    concepts::{Action, Filter, Match, MatchTime, Pattern, Time},
    protocol::{Order, PatternStatus},
    treedb::{
        helpers::{to_match, to_matchtime, to_time, to_u64},
        Database, Tree,
    },
};

use super::shutdown::ShutdownToken;

/// Responsible for handling all runtime logic dedicated to a [`Filter`].
/// Notably handles incoming lines from [`super::stream::stream_manager`]
/// and orders from the [`super::socket::socket_manager`]
#[derive(Clone)]
pub struct FilterManager {
    /// the Filter managed
    filter: &'static Filter,
    /// Permits to limit concurrency of actions execution
    exec_limit: Option<Arc<Semaphore>>,
    /// Permits to run pending actions on shutdown
    shutdown: ShutdownToken,
    /// Inner state.
    /// Protected by a [`Mutex`], permitting FilterManager to be cloned
    /// and concurrently owned by its stream manager, the socket manager,
    /// and actions' delayed tasks.
    state: Arc<Mutex<State>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum React {
    NoMatch,
    Match,
    Exec,
}

#[allow(clippy::unwrap_used)]
impl FilterManager {
    pub fn new(
        filter: &'static Filter,
        exec_limit: Option<Arc<Semaphore>>,
        shutdown: ShutdownToken,
        db: &mut Database,
        now: Time,
    ) -> Result<Self, String> {
        let this = Self {
            filter,
            exec_limit,
            shutdown,
            state: Arc::new(Mutex::new(State::new(
                filter,
                !filter.longuest_action_duration().is_zero(),
                db,
                now,
            )?)),
        };
        this.clear_past_triggers_and_schedule_future_actions(now);
        Ok(this)
    }

    pub fn handle_line(&self, line: &str, now: Time) -> React {
        if let Some(match_) = self.filter.get_match(line) {
            if self.handle_match(match_, now) {
                React::Exec
            } else {
                React::Match
            }
        } else {
            React::NoMatch
        }
    }

    fn handle_match(&self, m: Match, now: Time) -> bool {
        #[allow(clippy::unwrap_used)] // propagating panics is ok
        let mut state = self.state.lock().unwrap();
        state.clear_past_matches(now);

        let exec = match self.filter.retry() {
            None => true,
            Some(retry) => {
                state.add_match(m.clone(), now);
                // Number of stored times for this match >= configured retry for this filter
                state.get_times(&m) >= retry as usize
            }
        };

        if exec {
            state.remove_match(&m);
            state.add_trigger(m.clone(), now);
            self.schedule_exec(m, now, now, &mut state, false);
        }

        exec
    }

    pub fn handle_trigger(&self, patterns: BTreeMap<Arc<Pattern>, String>, now: Time) -> Result<(), String> {
        let match_ = self.filter.get_match_from_patterns(patterns)?;

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        let mut state = self.state.lock().unwrap();
        state.remove_match(&match_);
        state.add_trigger(match_.clone(), now);
        self.schedule_exec(match_, now, now, &mut state, false);

        Ok(())
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

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        let mut state = self.state.lock().unwrap();

        let mut cs: BTreeMap<_, _> = {
            let cloned_matches = state
                .matches
                .keys()
                // match filtering
                .filter(|match_| is_match(match_))
                // clone necessary to drop all references to State
                .cloned()
                .collect::<Vec<_>>();

            cloned_matches
                .into_iter()
                .map(|match_| {
                    // mutable State required here
                    if let Order::Flush = order {
                        state.remove_match(&match_);
                    }
                    let matches = state
                        .matches
                        .get(&match_)
                        .map(|times| times.len())
                        .unwrap_or(0);
                    (
                        match_,
                        PatternStatus {
                            matches,
                            ..Default::default()
                        },
                    )
                })
                .collect()
        };

        let cloned_triggers = state
            .triggers
            .keys()
            // match filtering
            .filter(|match_| is_match(&match_.m))
            // clone necessary to drop all references to State
            .cloned()
            .collect::<Vec<_>>();

        for mt in cloned_triggers.into_iter() {
            // mutable State required here
            // Remove the match from the triggers
            if let Order::Flush = order {
                // delete specific (Match, Time) tuple
                state.remove_trigger(&mt.m, &mt.t);
            }

            let m = mt.m.clone();
            let pattern_status = cs.entry(m).or_default();

            for action in self.filter.actions().values() {
                let action_time = mt.t + action.after_duration().unwrap_or_default();
                if action_time > now {
                    // Insert action
                    pattern_status
                        .actions
                        .entry(action.name().into())
                        .or_default()
                        .push(action_time.to_rfc3339().chars().take(19).collect());

                    // Execute the action early
                    if let Order::Flush = order {
                        exec_now(&self.exec_limit, action, mt.m.clone());
                    }
                }
            }
        }

        cs.into_iter().map(|(k, v)| (k.join(" "), v)).collect()
    }

    /// Schedule execution for a given Match.
    /// We check first if the trigger is still here
    /// because pending actions can be flushed.
    fn schedule_exec(
        &self,
        m: Match,
        t: Time,
        now: Time,
        state: &mut MutexGuard<State>,
        startup: bool,
    ) {
        for action in self
            .filter
            .actions()
            .values()
            // On startup, skip oneshot actions
            .filter(|action| !startup || !action.oneshot())
        {
            let exec_time = t + action.after_duration().unwrap_or_default();
            let m = m.clone();

            if exec_time <= now {
                if state.decrement_trigger(&m, t) {
                    exec_now(&self.exec_limit, action, m);
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
                    if !exiting || action.on_exit() {
                        #[allow(clippy::unwrap_used)] // propagating panics is ok
                        let mut state = this.state.lock().unwrap();
                        if state.decrement_trigger(&m, t) {
                            exec_now(&this.exec_limit, action, m);
                        }
                    }
                });
            }
        }
    }

    fn clear_past_triggers_and_schedule_future_actions(&self, now: Time) {
        let longuest_action_duration = self.filter.longuest_action_duration();
        let number_of_actions = self
            .filter
            .actions()
            .values()
            // On startup, skip oneshot actions
            .filter(|action| !action.oneshot())
            .count();

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        let mut state = self.state.lock().unwrap();

        let cloned_triggers = state
            .triggers
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect::<Vec<_>>();

        for (mt, remaining) in cloned_triggers.into_iter() {
            if remaining > 0 && mt.t + longuest_action_duration > now {
                // Insert back the upcoming times
                state.triggers.insert(mt.clone(), number_of_actions as u64);
                // Schedule the upcoming times
                self.schedule_exec(mt.m, mt.t, now, &mut state, true);
            } else {
                state.triggers.remove(&mt);
            }
        }
    }
}

fn exec_now(exec_limit: &Option<Arc<Semaphore>>, action: &'static Action, m: Match) {
    let exec_limit = exec_limit.clone();
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

fn filter_ordered_times_db_name(filter: &Filter) -> String {
    format!(
        "filter_ordered_times_{}.{}",
        filter.stream_name(),
        filter.name()
    )
}

fn filter_triggers_db_name(filter: &Filter) -> String {
    format!("filter_triggers_{}.{}", filter.stream_name(), filter.name())
}

/// Internal state of a [`FilterManager`].
/// Holds all data on current matches and triggers.
struct State {
    /// the Filter managed
    filter: &'static Filter,
    /// Has the filter at least an action with an after directive?
    has_after: bool,
    /// Saves all the current Matches for this Filter
    /// Has duplicate values for a key
    /// Not persisted
    matches: BTreeMap<Match, BTreeSet<Time>>,
    /// Alternative view of the current Matches for O(1) cleaning of old Matches
    /// without added async Tasks to remove them
    /// Persisted
    ordered_times: Tree<Time, Match>,
    /// Saves all the current Triggers for this Filter
    /// Persisted
    triggers: Tree<MatchTime, u64>,
}

impl State {
    fn new(
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

    fn add_match(&mut self, m: Match, t: Time) {
        let set = self.matches.entry(m.clone()).or_default();
        set.insert(t);
        self.ordered_times.insert(t, m);
    }

    fn add_trigger(&mut self, m: Match, t: Time) {
        // We record triggered filters only when there is an action with an `after` directive
        if self.has_after {
            // Add the (Match, Time) to the triggers map
            self.triggers
                .insert(MatchTime { m, t }, self.filter.actions().len() as u64);
        }
    }

    // Completely remove a Match from the matches
    fn remove_match(&mut self, m: &Match) {
        if let Some(set) = self.matches.get(m) {
            for t in set {
                self.ordered_times.remove(t);
            }
            self.matches.remove(m);
        }
    }

    /// Completely remove a Match from the triggers
    fn remove_trigger(&mut self, m: &Match, t: &Time) {
        self.triggers.remove(&MatchTime {
            m: m.clone(),
            t: *t,
        });
    }

    /// Returns whether we should still execute an action for this (Match, Time) trigger
    fn decrement_trigger(&mut self, m: &Match, t: Time) -> bool {
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

    fn clear_past_matches(&mut self, now: Time) {
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

    fn get_times(&self, m: &Match) -> usize {
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
