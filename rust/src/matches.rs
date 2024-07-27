use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        mpsc::{Receiver, Sender},
        Arc,
    },
    time::SystemTime,
};

use chrono::TimeDelta;
use timer::MessageTimer;

use crate::{
    config::Config,
    filter::FilterName,
    messages::{Match, MAT, MFT},
};

#[derive(Clone)]
pub enum MatchManagerInput {
    Match(MFT),
    Unmatch(MFT),
    #[allow(dead_code)]
    Flush(MFT),
}

type MatchesMap = BTreeMap<Match, BTreeMap<FilterName, BTreeSet<SystemTime>>>;

trait MatchesMapTrait {
    fn add(&mut self, mft: &MFT);
    fn rm(&mut self, mft: &MFT);
}
impl MatchesMapTrait for MatchesMap {
    fn add(&mut self, mft: &MFT) {
        let inner_map = self.entry(mft.m.clone()).or_default();
        let inner_set = inner_map.entry(mft.f.clone()).or_default();
        inner_set.insert(mft.t);
    }

    fn rm(&mut self, mft: &MFT) {
        if let Some(inner_map) = self.get_mut(&mft.m) {
            if let Some(inner_set) = inner_map.get_mut(&mft.f) {
                inner_set.remove(&mft.t);
                if inner_set.is_empty() {
                    inner_map.remove(&mft.f);
                }
            }
            if inner_map.is_empty() {
                self.remove(&mft.m);
            }
        }
    }
}

pub fn matches_manager(
    config: Arc<Config>,
    match_rx: Receiver<MatchManagerInput>,
    match_tx: Sender<MatchManagerInput>,
    action_tx: Sender<MAT>,
) {
    let mut matches: MatchesMap = BTreeMap::new();

    let timer = MessageTimer::new(match_tx);

    while let Ok(mft) = match_rx.recv() {
        match mft {
            MatchManagerInput::Match(mft) => {
                let filter = config.get_filter(&mft.f).unwrap();

                let is_retry = filter.retry().is_some();

                // Store matches
                if is_retry {
                    // Add new match
                    matches.add(&mft);
                    // Remove match when expired
                    timer.schedule_with_delay(
                        // unwrap: retry_duration is guaranted to be Some when retry is Some
                        // unwrap: TimeDelta max value is ~292471 millenaries
                        TimeDelta::from_std(filter.retry_duration().unwrap()).unwrap(),
                        MatchManagerInput::Unmatch(mft.clone()),
                    );
                }

                // Executing actions
                let exec = !is_retry || matches.get(&mft.m).map(|map| map.get(&mft.f)).is_some();
                if exec {
                    // Delete matches only if storing them
                    if is_retry {
                        matches.get_mut(&mft.m).unwrap().remove(&mft.f);
                    }
                    filter.send_actions(&mft.m, mft.t, &action_tx);
                }

                // TODO send to DB
                // log_tx.send(LogEntry { mft, exec });
            }
            MatchManagerInput::Unmatch(mft) => matches.rm(&mft),
            #[allow(clippy::todo)]
            MatchManagerInput::Flush(_) => todo!(), // TODO handle flushes
        }
    }
}
