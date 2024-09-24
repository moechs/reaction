use std::{
    collections::{BTreeMap, BTreeSet},
    sync::mpsc::{Receiver, Sender, SyncSender},
};

use log::debug;
use timer::MessageTimer;

use crate::{
    database::DatabaseManagerInput,
    execs::ExecsManagerInput,
    filter::Filter,
    messages::{LogEntry, Match, Time, MFT},
};

#[derive(Clone)]
pub enum MatchManagerInput {
    Match(MFT),
    Unmatch(MFT),
    #[allow(dead_code)]
    Flush(MFT),
    EndOfStartup,
    Stop,
}

type MatchesMap = BTreeMap<&'static Filter, BTreeMap<Match, BTreeSet<Time>>>;

// This trait is needed to permit to implement methods on an external type
trait MatchesMapTrait {
    fn add(&mut self, mft: &MFT);
    fn rm(&mut self, mft: &MFT);
    fn rm_times(&mut self, mft: &MFT);
    fn get_times(&self, mft: &MFT) -> usize;
}
impl MatchesMapTrait for MatchesMap {
    fn add(&mut self, mft: &MFT) {
        let inner_map = self.entry(mft.f).or_default();
        let inner_set = inner_map.entry(mft.m.clone()).or_default();
        inner_set.insert(mft.t);
    }

    fn rm(&mut self, mft: &MFT) {
        if let Some(inner_map) = self.get_mut(&mft.f) {
            if let Some(inner_set) = inner_map.get_mut(&mft.m) {
                inner_set.remove(&mft.t);
                if inner_set.is_empty() {
                    inner_map.remove(&mft.m);
                }
            }
            if inner_map.is_empty() {
                self.remove(&mft.f);
            }
        }
    }

    fn rm_times(&mut self, mft: &MFT) {
        if let Some(inner_map) = self.get_mut(&mft.f) {
            inner_map.remove(&mft.m);
            if inner_map.is_empty() {
                self.remove(&mft.f);
            }
        }
    }

    fn get_times(&self, mft: &MFT) -> usize {
        match self.get(&mft.f).and_then(|map| map.get(&mft.m)) {
            Some(x) => x.len(),
            None => 0,
        }
    }
}

pub fn matches_manager(
    match_rx: Receiver<MatchManagerInput>,
    match_tx: Sender<MatchManagerInput>,
    action_tx: Sender<ExecsManagerInput>,
    log_tx: SyncSender<DatabaseManagerInput>,
) {
    #[allow(clippy::mutable_key_type)] // Interior mutability of Arc is not used
    let mut matches: MatchesMap = BTreeMap::new();

    let timer = MessageTimer::new(match_tx);

    let mut startup = true;

    for mft in match_rx.iter() {
        match mft {
            MatchManagerInput::EndOfStartup => {
                debug!("end of startup!");
                startup = false;
            }
            MatchManagerInput::Match(mft) => {
                // Store matches
                let exec = match mft.f.retry() {
                    None => true,
                    Some(retry) => {
                        // Add new match
                        matches.add(&mft);
                        // Remove match when expired
                        let guard = timer.schedule_with_delay(
                            mft.f.retry_duration().unwrap(),
                            MatchManagerInput::Unmatch(mft.clone()),
                        );
                        guard.ignore();

                        matches.get_times(&mft) >= retry as usize
                    }
                };

                // Executing actions
                if exec {
                    // Delete matches only if storing them
                    if mft.f.retry().is_some() {
                        matches.rm_times(&mft);
                    }
                    mft.f.send_actions(&mft.m, mft.t, &action_tx);
                }

                if !startup {
                    log_tx
                        .send(DatabaseManagerInput::Log(LogEntry {
                            exec,
                            m: mft.m,
                            f: mft.f,
                            t: mft.t,
                        }))
                        .unwrap();
                }
            }
            MatchManagerInput::Unmatch(mft) => matches.rm(&mft),
            #[allow(clippy::todo)]
            MatchManagerInput::Flush(_) => todo!(), // TODO handle flushes
            MatchManagerInput::Stop => break,
        }
    }
}
