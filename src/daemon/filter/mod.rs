#[cfg(test)]
pub mod tests;

mod state;

use std::{
    collections::BTreeMap,
    process::Stdio,
    sync::{Arc, Mutex, MutexGuard},
};

use regex::Regex;
use tokio::sync::Semaphore;
use tracing::{error, info};

use crate::{
    concepts::{Action, Duplicate, Filter, Match, Pattern, PatternType, Time},
    protocol::{Order, PatternStatus},
    treedb::Database,
};

use state::State;

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

/// The react to a line handling.
#[derive(Debug, PartialEq, Eq)]
pub enum React {
    /// This line doesn't match
    NoMatch,
    /// This line matches, but no execution is triggered
    Match,
    /// This line matches, and an execution is triggered
    Trigger,
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
            state: Arc::new(Mutex::new(State::new(filter, db, now)?)),
        };
        this.clear_past_triggers_and_schedule_future_actions(now);
        Ok(this)
    }

    pub fn handle_line(&self, line: &str, now: Time) -> React {
        if let Some(match_) = self.filter.get_match(line) {
            if self.handle_match(match_, now) {
                React::Trigger
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

        let already_triggered = state.triggers.contains_key(&m);

        // if duplicate: ignore and already triggered, skip
        if already_triggered && Duplicate::Ignore == self.filter.duplicate() {
            return false;
        }

        let trigger = match self.filter.retry() {
            None => true,
            Some(retry) => {
                state.add_match(m.clone(), now);
                // Number of stored times for this match >= configured retry for this filter
                state.get_times(&m) >= retry as usize
            }
        };

        if trigger {
            state.remove_match(&m);
            let extend = already_triggered && Duplicate::Extend == self.filter.duplicate();
            if extend {
                state.remove_trigger(&m);
            }
            state.add_trigger(m.clone(), now);
            self.schedule_exec(m, now, now, &mut state, false, extend);
        }

        trigger
    }

    pub fn handle_trigger(
        &self,
        patterns: BTreeMap<Arc<Pattern>, String>,
        now: Time,
    ) -> Result<(), String> {
        let match_ = self.filter.get_match_from_patterns(patterns)?;

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        let mut state = self.state.lock().unwrap();
        state.remove_match(&match_);
        state.add_trigger(match_.clone(), now);
        self.schedule_exec(match_, now, now, &mut state, false, false);

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
            .filter(|match_| is_match(match_))
            // clone necessary to drop all references to State
            .cloned()
            .collect::<Vec<_>>();

        for m in cloned_triggers.into_iter() {
            let map = state.triggers.get(&m).unwrap().clone();

            if let Order::Flush = order {
                state.remove_trigger(&m);
            }

            for (t, remaining) in map {
                if remaining > 0 {
                    let pattern_status = cs.entry(m.clone()).or_default();

                    for action in self.filter.actions().values() {
                        let action_time = t + action.after_duration().unwrap_or_default();
                        if action_time > now {
                            // Insert action
                            pattern_status
                                .actions
                                .entry(action.name().into())
                                .or_default()
                                .push(action_time.to_rfc3339().chars().take(19).collect());

                            // Execute the action early
                            if let Order::Flush = order {
                                exec_now(&self.exec_limit, action, m.clone());
                            }
                        }
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
        only_after: bool,
    ) {
        // Testing if we have an IPv4 or IPv6
        let ip_type = if self.filter.check_ip() {
            self.filter
                .patterns()
                .iter()
                .zip(&m)
                .find(|(p, _)| p.pattern_type() == PatternType::Ip)
                .map(|(_, m)| -> _ {
                    // Using this dumb heuristic is ok,
                    // because we know we have a valid IP address.
                    if m.contains(':') {
                        PatternType::Ipv6
                    } else if m.contains('.') {
                        PatternType::Ipv4
                    } else {
                        PatternType::Regex
                    }
                })
                .unwrap_or(PatternType::Regex)
        } else {
            PatternType::Regex
        };

        // Scheduling each action
        for action in self
            .filter
            .actions()
            .values()
            // On startup, skip oneshot actions
            .filter(|action| !startup || !action.oneshot())
            // If only_after, keep only after actions
            .filter(|action| !only_after || action.after_duration().is_some())
            // If specific ip version, check it
            .filter(|action| !action.ipv4only() || ip_type == PatternType::Ipv4)
            .filter(|action| !action.ipv6only() || ip_type == PatternType::Ipv6)
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
            .count() as u64;

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        let mut state = self.state.lock().unwrap();

        let cloned_triggers = state
            .triggers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>();

        for (m, map) in cloned_triggers.into_iter() {
            let mut new_map = BTreeMap::default();
            for (t, remaining) in map.into_iter() {
                if remaining > 0 && t + longuest_action_duration > now {
                    // Insert back the upcoming times
                    new_map.insert(t, number_of_actions);
                    // Schedule the upcoming times
                    self.schedule_exec(m.clone(), t, now, &mut state, true, false);
                }
            }
            if new_map.is_empty() {
                state.triggers.remove(&m);
            } else {
                state.triggers.insert(m, new_map);
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

impl PartialEq for FilterManager {
    fn eq(&self, other: &Self) -> bool {
        self.filter == other.filter
    }
}
impl Eq for FilterManager {}

impl Ord for FilterManager {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.filter.cmp(other.filter)
    }
}
impl PartialOrd for FilterManager {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
