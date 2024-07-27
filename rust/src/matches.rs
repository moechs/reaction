use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        mpsc::{Receiver, Sender},
        Arc,
    },
    time::SystemTime,
};

use crate::{
    config::Config,
    filter::FilterName,
    messages::{Match, MAT, MFT},
};

type MatchesMap = BTreeMap<Match, BTreeMap<FilterName, BTreeSet<SystemTime>>>;

// TODO handle flushes
pub fn matches_manager(config: Arc<Config>, match_rx: Receiver<MFT>, action_tx: Sender<MAT>) {
    let mut matches: MatchesMap = BTreeMap::new();

    while let Ok(mft) = match_rx.recv() {
        let filter = config.get_filter(&mft.f).unwrap();

        let is_retry = filter.retry.is_some();

        // Store matches
        if is_retry {
            // Make sure collections exist
            let inner_map = matches.entry(mft.m.clone()).or_default();
            let inner_set = inner_map.entry(mft.f.clone()).or_default();
            // Add new match
            inner_set.insert(mft.t);
            // Remove match when expired
            // TODO timer that only sends it back to matches_manager
            // https://docs.rs/timer/latest/timer/struct.Timer.html
            // replace Receiver<MFT> with Receiver<enum { Match(MFT), Unmatch(MFT), Flush(MFT) }>
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
}
