#[cfg(test)]
pub mod tests;

mod state;

use std::{collections::BTreeMap, process::Stdio, sync::Arc};

use chrono::TimeZone;
use reaction_plugin::{ActionImpl, shutdown::ShutdownToken};
use regex::Regex;
use tokio::sync::{Mutex, MutexGuard, Semaphore};
use tracing::{debug, error, info};

use crate::{
    concepts::{Action, Duplicate, Filter, Match, Pattern, Time},
    daemon::plugin::Plugins,
    protocol::{Order, PatternStatus},
    treedb::Database,
};

use state::State;

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
    /// Action Plugins
    action_plugins: BTreeMap<&'static String, ActionImpl>,
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
    pub async fn new(
        filter: &'static Filter,
        exec_limit: Option<Arc<Semaphore>>,
        shutdown: ShutdownToken,
        db: &mut Database,
        plugins: &mut Plugins,
        now: Time,
    ) -> Result<Self, String> {
        let mut action_plugins = BTreeMap::default();
        for (action_name, action) in filter.actions.iter().filter(|action| action.1.is_plugin()) {
            action_plugins.insert(
                action_name,
                plugins
                    .init_action_impl(
                        action.stream_name.clone(),
                        action.filter_name.clone(),
                        action.name.clone(),
                        action.action_type.clone().unwrap(),
                        action.options.clone(),
                        action
                            .patterns
                            .iter()
                            .map(|pattern| pattern.name.clone())
                            .collect(),
                    )
                    .await?,
            );
            debug!(
                "successfully intialized action {}.{}.{}",
                action.stream_name, action.filter_name, action.name
            );
        }
        let this = Self {
            filter,
            exec_limit,
            shutdown,
            action_plugins,
            state: Arc::new(Mutex::new(State::new(filter, db, now).await?)),
        };
        Ok(this)
    }

    pub async fn handle_line(&self, line: &str, now: Time) -> React {
        if let Some(match_) = self.filter.get_match(line) {
            if self.handle_match(match_, now).await {
                React::Trigger
            } else {
                React::Match
            }
        } else {
            React::NoMatch
        }
    }

    async fn handle_match(&self, m: Match, now: Time) -> bool {
        #[allow(clippy::unwrap_used)] // propagating panics is ok
        let mut state = self.state.lock().await;
        state.clear_past_matches(now).await;

        // if Duplicate::Ignore and already triggered, skip
        if state.triggers.contains_key(&m) && Duplicate::Ignore == self.filter.duplicate {
            return false;
        }

        info!("{}: match {:?}", self.filter, &m);

        let trigger = match self.filter.retry {
            None => true,
            Some(retry) => {
                state.add_match(m.clone(), now).await;
                // Number of stored times for this match >= configured retry for this filter
                state.get_times(&m).await >= retry as usize
            }
        };

        if trigger {
            state.remove_match(&m).await;
            let actions_left = if Duplicate::Extend == self.filter.duplicate {
                // Get number of actions left from last trigger
                state
                    .remove_trigger(&m)
                    .await
                    // Only one entry in the map because Duplicate::Extend
                    .and_then(|map| map.first_key_value().map(|(_, n)| *n))
            } else {
                None
            };
            state.add_trigger(m.clone(), now, actions_left).await;
            self.schedule_exec(m, now, now, &mut state, false, actions_left)
                .await;
        }

        trigger
    }

    pub async fn handle_trigger(
        &self,
        patterns: BTreeMap<Arc<Pattern>, String>,
        now: Time,
    ) -> Result<(), String> {
        let match_ = self.filter.get_match_from_patterns(patterns)?;

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        let mut state = self.state.lock().await;
        state.remove_match(&match_).await;
        state.add_trigger(match_.clone(), now, None).await;
        self.schedule_exec(match_, now, now, &mut state, false, None)
            .await;

        Ok(())
    }

    pub async fn handle_order(
        &self,
        patterns: &BTreeMap<Arc<Pattern>, Regex>,
        order: Order,
        now: Time,
    ) -> BTreeMap<String, PatternStatus> {
        let is_match = |match_: &Match| {
            match_
                .iter()
                .zip(self.filter.patterns.as_ref())
                .filter_map(|(a_match, pattern)| {
                    patterns.get(pattern.as_ref()).map(|regex| (a_match, regex))
                })
                .all(|(a_match, regex)| regex.is_match(a_match))
        };

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        let mut state = self.state.lock().await;

        let mut cs: BTreeMap<_, _> = {
            let cloned_matches = state
                .matches
                .keys()
                // match filtering
                .filter(|match_| is_match(match_))
                // clone necessary to drop all references to State
                .cloned()
                .collect::<Vec<_>>();

            let mut cs = BTreeMap::new();
            for match_ in cloned_matches {
                // mutable State required here
                if let Order::Flush = order {
                    state.remove_match(&match_).await;
                }
                let matches = state
                    .matches
                    .get(&match_)
                    .map(|times| times.len())
                    .unwrap_or(0);
                cs.insert(
                    match_,
                    PatternStatus {
                        matches,
                        ..Default::default()
                    },
                );
            }
            cs
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
                state.remove_trigger(&m).await;
            }

            for (t, remaining) in map {
                if remaining > 0 {
                    let pattern_status = cs.entry(m.clone()).or_default();

                    for action in self.filter.filtered_actions_from_match(&m) {
                        let action_time = t + action.after_duration.unwrap_or_default();
                        if action_time > now {
                            // Pretty print time
                            let time = chrono::Local
                                .timestamp_opt(
                                    action_time.as_secs() as i64,
                                    action_time.subsec_nanos(),
                                )
                                .unwrap()
                                .to_rfc3339()
                                .chars()
                                .take(19)
                                .collect();
                            // Insert action
                            pattern_status
                                .actions
                                .entry(action.name.clone())
                                .or_default()
                                .push(time);

                            // Execute the action early
                            if let Order::Flush = order {
                                self.exec_now(action, m.clone(), t);
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
    async fn schedule_exec(
        &self,
        m: Match,
        t: Time,
        now: Time,
        state: &mut MutexGuard<'_, State>,
        startup: bool,
        actions_left: Option<u64>,
    ) {
        let actions = self
            .filter
            .filtered_actions_from_match(&m)
            .into_iter()
            // On startup, skip oneshot actions
            .filter(|action| !startup || !action.oneshot)
            // skip any actions
            .skip(match actions_left {
                Some(actions_left) => {
                    self.filter.filtered_actions_from_match(&m).len() - actions_left as usize
                }
                None => 0,
            });

        // Scheduling each action
        for action in actions {
            let exec_time = t + action.after_duration.unwrap_or_default();
            let m = m.clone();

            if exec_time <= now {
                if state.decrement_trigger(&m, t, false).await {
                    self.exec_now(action, m, t);
                }
            } else {
                let this = self.clone();
                let action_impl = self.action_plugins.get(&action.name).cloned();
                tokio::spawn(async move {
                    let dur = exec_time - now;
                    // Wait either for end of sleep
                    // or reaction exiting
                    let exiting = tokio::select! {
                        _ = tokio::time::sleep(dur.into()) => false,
                        _ = this.shutdown.wait() => true,
                    };
                    // Exec action if triggered hasn't been already flushed
                    if !exiting || action.on_exit {
                        #[allow(clippy::unwrap_used)] // propagating panics is ok
                        let mut state = this.state.lock().await;
                        if state.decrement_trigger(&m, t, exiting).await {
                            exec_now(&this.exec_limit, this.shutdown, action, action_impl, m, t);
                        }
                    }
                });
            }
        }
    }

    /// Clear past triggers and schedule future actions
    pub async fn start(&self, now: Time) {
        let longuest_action_duration = self.filter.longuest_action_duration;
        let number_of_actions = self
            .filter
            .actions
            .values()
            // On startup, skip oneshot actions
            .filter(|action| !action.oneshot)
            .count() as u64;

        #[allow(clippy::unwrap_used)] // propagating panics is ok
        let mut state = self.state.lock().await;

        let cloned_triggers = state
            .triggers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>();

        for (m, map) in cloned_triggers.into_iter() {
            let map: BTreeMap<_, _> = map
                .into_iter()
                // Keep only up-to-date triggers
                .filter(|(t, remaining)| *remaining > 0 && *t + longuest_action_duration > now)
                // Reset action count
                .map(|(t, _)| (t, number_of_actions))
                .collect();

            if map.is_empty() {
                state.triggers.remove(&m).await;
            } else {
                // Filter duplicates
                // unwrap is fine because map is not empty (see if)
                let map = match self.filter.duplicate {
                    // Keep only last item
                    Duplicate::Extend => BTreeMap::from([map.into_iter().next_back().unwrap()]),
                    // Keep only first item
                    Duplicate::Ignore => BTreeMap::from([map.into_iter().next().unwrap()]),
                    // No filtering
                    Duplicate::Rerun => map,
                };
                state.triggers.insert(m.clone(), map.clone()).await;
                for (t, _) in map {
                    // Schedule the upcoming times
                    self.schedule_exec(m.clone(), t, now, &mut state, true, None)
                        .await;
                }
            }
        }
    }

    fn exec_now(&self, action: &'static Action, m: Match, t: Time) {
        let action_impl = self.action_plugins.get(&action.name).cloned();
        exec_now(
            &self.exec_limit,
            self.shutdown.clone(),
            action,
            action_impl,
            m,
            t,
        )
    }
}

fn exec_now(
    exec_limit: &Option<Arc<Semaphore>>,
    shutdown: ShutdownToken,
    action: &'static Action,
    action_impl: Option<ActionImpl>,
    m: Match,
    t: Time,
) {
    let exec_limit = exec_limit.clone();
    tokio::spawn(async move {
        // Move ShutdownToken in task
        let _shutdown = shutdown;

        match action_impl {
            Some(action_impl) => {
                info!(
                    "{action}: run {} [{:?}]",
                    action.action_type.clone().unwrap_or_default(),
                    &m,
                );

                // Sending action
                let (response_tx, response_rx) = remoc::rch::oneshot::channel();
                if let Err(err) = action_impl
                    .tx
                    .send(reaction_plugin::Exec {
                        match_: m,
                        result: response_tx,
                        time: t.into(),
                    })
                    .await
                {
                    error!("{action}: communication with plugin failed: {err}");
                    return;
                }

                // Receiving response
                match response_rx.await {
                    Ok(Ok(())) => (),
                    Ok(Err(err)) => error!(
                        "{action}: run {}: {err}",
                        action.action_type.clone().unwrap_or_default(),
                    ),
                    Err(err) => error!("{action}: communication with plugin failed: {err}"),
                }
            }
            None => {
                // Wait for semaphore's permission, if it is Some
                let _permit = match exec_limit {
                    #[allow(clippy::unwrap_used)] // We know the semaphore is not closed
                    Some(semaphore) => Some(semaphore.acquire_owned().await.unwrap()),
                    None => None,
                };

                // Construct command
                let mut command = action.exec(&m);

                info!("{action}: run [{:?}]", command.as_std());
                if let Err(err) = command
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .stdout(Stdio::piped())
                    .status()
                    .await
                {
                    error!("{action}: run [{:?}], code {err}", command.as_std());
                }
            }
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
