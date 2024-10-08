use std::collections::BTreeMap;

use chrono::Local;
use log::debug;
use tokio::{
    sync::{mpsc, oneshot},
    time,
};

use super::{
    database::DatabaseManagerInput,
    statemap::{FilterOptions, StateMap, StateMapTrait},
    ExecsManagerInput,
};
use crate::concepts::{ActionFilter, Filter, LogEntry, Order, MFT};

#[derive(Debug)]
pub enum MatchManagerInput {
    Match(MFT),
    Unmatch(MFT),
    Order(Order, FilterOptions, oneshot::Sender<MatchesMap>),
    EndOfStartup,
    Stop,
}

pub type MatchesMap = StateMap<Filter>;

pub async fn matches_manager(
    mut match_rx: mpsc::Receiver<MatchManagerInput>,
    match_tx: mpsc::Sender<MatchManagerInput>,
    action_tx: mpsc::Sender<ExecsManagerInput>,
    log_tx: mpsc::Sender<DatabaseManagerInput>,
) {
    let mut matches: MatchesMap = BTreeMap::new();

    let mut startup = true;

    while let Some(mft) = match_rx.recv().await {
        for (filter, map) in matches.iter() {
            debug!("MATCHES {:?} {:?}", filter.full_name(), map.keys());
        }
        match mft {
            MatchManagerInput::EndOfStartup => {
                debug!("end of startup!");
                startup = false;
            }
            MatchManagerInput::Match(mft) => {
                // Store matches
                let exec = match mft.o.retry() {
                    None => true,
                    Some(retry) => {
                        // Add new match
                        matches.add(&mft);
                        // Remove match when expired
                        {
                            let mft = mft.clone();
                            let match_tx = match_tx.clone();
                            tokio::spawn(async move {
                                #[allow(clippy::unwrap_used)]
                                // retry_duration is always Some() after filter's setup
                                let dur = mft.t - Local::now() + mft.o.retry_duration().unwrap();
                                time::sleep(dur.to_std().unwrap()).await;
                                #[allow(clippy::unwrap_used)] // propagating panics is ok
                                match_tx
                                    .send(MatchManagerInput::Unmatch(mft))
                                    .await
                                    .unwrap();
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
                    mft.o.send_actions(&mft.m, mft.t, &action_tx).await;
                }

                if !startup {
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
            }
            MatchManagerInput::Unmatch(mft) => {
                matches.rm(&mft);
            }
            MatchManagerInput::Order(order, options, tx) => {
                let filtered = matches.filtered(options);

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
            #[allow(clippy::unwrap_used)] // propagating panics is ok
            MatchManagerInput::Stop => break,
        }
    }
}
