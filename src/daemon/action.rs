use std::{
    collections::{BTreeMap, BTreeSet},
    process::Stdio,
    sync::{Arc, Mutex},
};

use chrono::{Local, TimeDelta};
use tokio::sync::Semaphore;
use tracing::{error, info};

use crate::{
    concepts::{Action, Match, Time},
    protocol::Order,
};

struct State {
    pending: BTreeMap<Match, BTreeSet<Time>>,
    ordered_times: BTreeMap<Time, Match>,
}

impl State {
    fn add_match(&mut self, m: &Match, t: &Time) {
        self.pending.entry(m.clone()).or_default().insert(*t);
        self.ordered_times.insert(*t, m.clone());
    }

    fn remove(&mut self, m: &Match, t: &Time) -> bool {
        self.pending.entry(m.clone()).and_modify(|times| {
            times.remove(t);
        });
        self.ordered_times.remove(t).is_some()
    }

    fn clear_past_times(&mut self, now: &Time, after: Option<TimeDelta>) {
        let after = after.unwrap_or_default();
        while self
            .ordered_times
            .first_key_value()
            .is_some_and(|(k, _)| *k + after < *now)
        {
            #[allow(clippy::unwrap_used)] // we just checked in the condition that first is_some
            let (_, m) = self.ordered_times.pop_first().unwrap();
            self.pending.remove(&m);
        }
    }
}

#[derive(Clone)]
pub struct ActionManager {
    action: &'static Action,
    exec_limit: Option<Arc<Semaphore>>,
    state: Arc<Mutex<State>>,
}

impl ActionManager {
    pub fn action(&self) -> &'static Action {
        self.action
    }

    pub fn new(
        action: &'static Action,
        pending: BTreeMap<Match, BTreeSet<Time>>,
        exec_limit: Option<Arc<Semaphore>>,
    ) -> Self {
        Self {
            action,
            exec_limit,
            state: Arc::new(Mutex::new(State {
                pending: pending.clone(),
                ordered_times: pending
                    .into_iter()
                    .flat_map(|(m, times)| times.into_iter().map(move |time| (time, m.clone())))
                    .collect(),
            })),
        }
    }

    pub fn handle_exec(&mut self, m: Match, t: Time) {
        let now = Local::now();
        let exec_t = t + self.action.after_duration().unwrap_or_default();
        if exec_t < now {
            self.exec_now(m);
        } else {
            {
                #[allow(clippy::unwrap_used)] // propagating panics is ok
                let mut state = self.state.lock().unwrap();
                state.clear_past_times(&t, self.action.after_duration());
                state.add_match(&m, &exec_t);
            }
            let this = self.clone();
            tokio::spawn(async move {
                let dur = (exec_t - now).to_std().expect("Duration is bigger than what's supported. Did you put an enormous after duration?");
                tokio::time::sleep(dur).await;
                #[allow(clippy::unwrap_used)] // propagating panics is ok
                let mut state = this.state.lock().unwrap();
                if state.remove(&m, &exec_t) {
                    this.exec_now(m);
                }
            });
        }
    }

    pub fn handle_order<F: Fn(&Match) -> bool>(
        &self,
        order: Order,
        is_match: F,
    ) -> BTreeMap<Vec<String>, Vec<String>> {
        #[allow(clippy::unwrap_used)] // propagating panics is ok
        let mut state = self.state.lock().unwrap();
        state
            .pending
            .clone()
            .into_iter()
            // match filtering
            .filter(|(match_, _)| is_match(match_))
            .fold(BTreeMap::default(), |mut acc, (match_, times)| {
                let times = times
                    .iter()
                    .map(|time| {
                        if let Order::Flush = order {
                            if state.remove(&match_, time) {
                                self.exec_now(match_.clone());
                            }
                        }
                        time.to_rfc3339().chars().take(19).collect()
                    })
                    .collect();
                acc.insert(match_, times);
                acc
            })
    }

    fn exec_now(&self, m: Match) {
        let semaphore = self.exec_limit.clone();
        let action = self.action;
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

    pub fn quit(&mut self) {
        if self.action.on_exit() {
            #[allow(clippy::unwrap_used)] // propagating panics is ok
            let mut state = self.state.lock().unwrap();
            for (m, times) in &state.pending {
                for _ in times {
                    self.exec_now(m.clone());
                }
            }
            state.pending.clear();
            state.ordered_times.clear();
        }
    }
}
