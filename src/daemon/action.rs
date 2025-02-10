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
    /// sled::Tree quivalent to BTreeMap<Match, BTreeSet<Time>>,
    pending: sled::Tree,
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
        };
        manager.clear_past_times_and_schedule_future_actions();
        Ok(manager)
    }

    pub fn handle_exec(&mut self, m: Match, t: Time) {
        let now = Local::now();
        let exec_t = t + self.action.after_duration().unwrap_or_default();
        if exec_t < now {
            self.exec_now(m);
        } else {
            self.add_match(&m, &exec_t);
            self.schedule_exec(m, exec_t, now);
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
                            if self.remove_match(&match_, time) {
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

    fn schedule_exec(&self, m: Match, t: Time, now: Time) {
        let this = self.clone();
        tokio::spawn(async move {
            let dur = (t - now).to_std().expect(
                "Duration is bigger than what's supported. Did you put an enormous after duration?",
            );
            tokio::time::sleep(dur).await;
            #[allow(clippy::unwrap_used)] // propagating panics is ok
            if this.remove_match(&m, &t) {
                this.exec_now(m);
            }
        });
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
        self.pending
            .fetch_and_update_ext::<Match, BTreeSet<Time>, _>(m, |set| {
                let mut set = set.unwrap_or_default();
                set.insert(*t);
                Some(set)
            })
            .unwrap();
    }

    fn remove_match(&self, m: &Match, t: &Time) -> bool {
        self.pending
            .fetch_and_update_ext::<Match, BTreeSet<Time>, _>(m, |set| {
                let mut set = set.unwrap_or_default();
                set.remove(t);
                Some(set)
            })
            .unwrap()
            .is_some()
    }

    fn clear_past_times_and_schedule_future_actions(&self) {
        let now = Local::now();
        for (m, set) in self.pending.iter_ext::<Match, BTreeSet<Time>>() {

            // Keep only times that are still future
            let new_set: BTreeSet<_> = set.into_iter().filter(|t| *t > now).collect();

            if new_set.is_empty() {
                // No upcoming time, delete the entry from the Tree
                self.pending.remove_ext::<Match, BTreeSet<Time>>(&m);
            } else {
                // Insert back the upcoming times
                let _ = self
                    .pending
                    .insert_ext::<Match, BTreeSet<Time>>(&m, &new_set);

                // Schedule the upcoming times
                for t in new_set.into_iter() {
                    self.schedule_exec(m.clone(), t, now);
                }
            }
        }
    }
}
