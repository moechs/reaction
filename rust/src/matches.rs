use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        mpsc::{Receiver, Sender},
        Arc,
    },
};

use timer::MessageTimer;

use crate::{
    config::Config,
    execs::ExecsManagerInput,
    filter::FilterName,
    messages::{Match, Time, MFT},
};

#[derive(Clone)]
pub enum MatchManagerInput {
    Match(MFT),
    Unmatch(MFT),
    #[allow(dead_code)]
    Flush(MFT),
}

type MatchesMap = BTreeMap<FilterName, BTreeMap<Match, BTreeSet<Time>>>;

trait MatchesMapTrait {
    fn add(&mut self, mft: &MFT);
    fn rm(&mut self, mft: &MFT);
}
impl MatchesMapTrait for MatchesMap {
    fn add(&mut self, mft: &MFT) {
        let inner_map = self.entry(mft.f.clone()).or_default();
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
}

pub fn matches_manager(
    config: Arc<Config>,
    match_rx: Receiver<MatchManagerInput>,
    match_tx: Sender<MatchManagerInput>,
    action_tx: Sender<ExecsManagerInput>,
) {
    let mut matches: MatchesMap = BTreeMap::new();

    let timer = MessageTimer::new(match_tx);

    while let Ok(mft) = match_rx.recv() {
        match mft {
            MatchManagerInput::Match(mft) => {
                let filter = config.get_filter(&mft.f.stream, &mft.f.filter).unwrap();

                let is_retry = filter.retry().is_some();

                // Store matches
                if is_retry {
                    // Add new match
                    matches.add(&mft);
                    // Remove match when expired
                    let guard = timer.schedule_with_delay(
                        filter.retry_duration().unwrap(),
                        MatchManagerInput::Unmatch(mft.clone()),
                    );
                    guard.ignore();
                }

                // Executing actions
                let exec = !is_retry || matches.get(&mft.f).map(|map| map.get(&mft.m)).is_some();
                if exec {
                    // Delete matches only if storing them
                    if is_retry {
                        matches.get_mut(&mft.f).unwrap().remove(&mft.m);
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
