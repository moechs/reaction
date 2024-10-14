use std::collections::BTreeMap;

use chrono::Local;
use tokio::{
    sync::{mpsc, watch},
    time,
};

use super::{
    database::DatabaseManagerInput,
    socket::SocketOrder,
    statemap::{StateMap, StateMapTrait},
};
use crate::concepts::{Filter, LogEntry, Order, MAT, MFT};

pub type MatchesMap = StateMap<Filter>;

pub async fn matches_manager(
    mut match_rx: mpsc::Receiver<MFT>,
    mut startup_match_rx: mpsc::Receiver<MFT>,
    mut socket_order_rx: mpsc::Receiver<SocketOrder<MatchesMap>>,
    action_tx: mpsc::Sender<MAT>,
    log_tx: mpsc::Sender<DatabaseManagerInput>,
    mut stop: watch::Receiver<bool>,
) {
    let mut matches: MatchesMap = BTreeMap::new();

    let (unmatches_tx, mut unmatches_rx) = mpsc::channel(1);

    while let Some(mft) = startup_match_rx.recv().await {
        let _ = handle_match(&mut matches, mft.clone(), &unmatches_tx, &action_tx, &stop).await;
    }

    loop {
        tokio::select! {
            _ = stop.changed() => break,
            Some(mft) = match_rx.recv() => {
                let exec = handle_match(&mut matches, mft.clone(), &unmatches_tx, &action_tx, &stop).await;

                #[allow(clippy::unwrap_used)] // propagating panics is ok
                log_tx
                    .send(DatabaseManagerInput::Log(LogEntry {
                        exec,
                        m: mft.m,
                        f: mft.o,
                        t: mft.t,
                    }))
                .await
                    .unwrap();
            }
            Some(mft) = unmatches_rx.recv() => {
                matches.rm(&mft);
            }
            Some((order, options, tx)) = socket_order_rx.recv() => {
                // FIXME do not clone
                let filtered = matches.clone().filtered(options);

                if let Order::Flush = order {
                    let now = Local::now();
                    // filter the state_map according to provided options
                    for (filter, inner_map) in &filtered {
                        for match_ in inner_map.keys() {
                            let mft = MFT {
                                m: match_.clone(),
                                o: filter,
                                t: now,
                            };
                            // delete them from state
                            matches.rm_times(&mft);
                            // send them to DB
                            #[allow(clippy::unwrap_used)] // propagating panics is ok
                            log_tx
                                .send(DatabaseManagerInput::Flush(LogEntry {
                                    exec: false,
                                    m: mft.m,
                                    f: mft.o,
                                    t: mft.t,
                                }))
                                .await
                                .unwrap();
                        }
                    }
                }

                #[allow(clippy::unwrap_used)] // propagating panics is ok
                tx.send(filtered).unwrap();
            }
            else => break
        }
    }
}

async fn handle_match(
    matches: &mut MatchesMap,
    mft: MFT,
    unmatches_tx: &mpsc::Sender<MFT>,
    action_tx: &mpsc::Sender<MAT>,
    stop: &watch::Receiver<bool>,
) -> bool {
    // Store matches
    let exec = match mft.o.retry() {
        None => true,
        Some(retry) => {
            // Add new match
            matches.add(&mft);
            // Remove match when expired
            {
                let mft = mft.clone();
                let unmatches_tx = unmatches_tx.clone();
                let mut stop = stop.clone();
                tokio::spawn(async move {
                    #[allow(clippy::unwrap_used)]
                    // retry_duration is always Some() after filter's setup
                    let dur = (mft.t - Local::now() + mft.o.retry_duration().unwrap())
                        .to_std()
                        .expect("Duration is bigger than what's supported. Did you put an enormous retry_duration?");
                    tokio::select! {
                        biased;
                        _ = stop.changed() => {}
                        _ = time::sleep(dur) => {
                            #[allow(clippy::unwrap_used)] // propagating panics is ok
                            unmatches_tx.send(mft).await.unwrap();
                        }
                    }
                });
            }

            matches.get_times(&mft) >= retry as usize
        }
    };

    // Executing actions
    if exec {
        // Delete matches only if storing them
        if mft.o.retry().is_some() {
            matches.rm_times(&mft);
        }
        mft.o.send_actions(&mft.m, mft.t, action_tx).await;
    }

    exec
}
