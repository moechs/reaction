use std::{
    collections::{BTreeMap, BTreeSet},
    process::Stdio,
    sync::Arc,
};

use chrono::Local;
use tokio::sync::Semaphore;
use tracing::{error, info};

use crate::{
    concepts::{Action, Match, Time},
    protocol::Order,
};

use super::{SledDbExt, SledTreeExt};

#[derive(Clone)]
pub struct ActionManager {
    action: &'static Action,
    exec_limit: Option<Arc<Semaphore>>,
    // BTreeMap<Match, BTreeSet<Time>>,
    pending: sled::Tree,
    // BTreeMap<Time, Match>,
    ordered_times: sled::Tree,
}

impl ActionManager {
    pub fn action(&self) -> &'static Action {
        self.action
    }

    pub fn new(
        action: &'static Action,
        exec_limit: Option<Arc<Semaphore>>,
        db: &sled::Db,
    ) -> Result<Self, sled::Error> {
        let manager = Self {
            action,
            exec_limit,
            pending: db.open_action_pending_tree(action)?,
            ordered_times: db.open_action_ordered_times_tree(action)?,
        };
        let now = Local::now();
        manager.clear_past_times(&now);
        Ok(manager)
    }

    pub fn handle_exec(&mut self, m: Match, t: Time) {
        let now = Local::now();
        let exec_t = t + self.action.after_duration().unwrap_or_default();
        if exec_t < now {
            self.exec_now(m);
        } else {
            // FIXME is clearing here buggy logic or right logic?
            self.clear_past_times(&t);
            self.add_match(&m, &exec_t);

            let this = self.clone();
            tokio::spawn(async move {
                let dur = (exec_t - now).to_std().expect("Duration is bigger than what's supported. Did you put an enormous after duration?");
                tokio::time::sleep(dur).await;
                #[allow(clippy::unwrap_used)] // propagating panics is ok
                if this.remove(&m, &exec_t) {
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
        self.pending
            .clone()
            .iter_ext::<Match, BTreeSet<Time>>()
            // match filtering
            .filter(|(match_, _)| is_match(match_))
            .fold(BTreeMap::default(), |mut acc, (match_, times)| {
                let times = times
                    .iter()
                    .map(|time| {
                        if let Order::Flush = order {
                            if self.remove(&match_, time) {
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
            self.pending
                .iter()
                .map(|elt| {
                    let (k, v) = elt.unwrap();
                    let k: Match = bincode::deserialize(&k).unwrap();
                    let v: BTreeSet<Time> = bincode::deserialize(&v).unwrap();
                    (k, v)
                })
                .for_each(|(m, times)| {
                    for _ in times {
                        self.exec_now(m.clone());
                    }
                });
        }
    }

    fn add_match(&self, m: &Match, t: &Time) {
        // FIXME do this in a transaction
        self.pending
            .fetch_and_update_ext::<Match, BTreeSet<Time>, _>(m, |set| {
                let mut set = set.unwrap_or_default();
                set.insert(*t);
                Some(set)
            })
            .unwrap();
        self.ordered_times.insert_ext::<Time, Match>(t, m).unwrap();
    }

    fn remove(&self, m: &Match, t: &Time) -> bool {
        self.pending
            .fetch_and_update_ext::<Match, BTreeSet<Time>, _>(m, |set| {
                let mut set = set.unwrap_or_default();
                set.remove(t);
                Some(set)
            })
            .unwrap();
        self.ordered_times.remove_ext::<Time, Match>(t).is_some()
    }

    fn clear_past_times(&self, now: &Time) {
        let after = self.action.after_duration().unwrap_or_default();
        while self
            .ordered_times
            .first_ext::<Time, Match>()
            .unwrap()
            .is_some_and(|(k, _)| k + after < *now)
        {
            #[allow(clippy::unwrap_used)] // second unwrap: we just checked in the condition that first is_some
            let (t, m) = self
                .ordered_times
                .pop_min_ext::<Time, Match>()
                .unwrap()
                .unwrap();
            self.pending
                .fetch_and_update_ext::<Match, BTreeSet<Time>, _>(&m, |set| {
                    let mut set = set.unwrap();
                    set.remove(&t);
                    Some(set)
                })
                .unwrap();
        }
    }
}
