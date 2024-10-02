use std::{
    collections::BTreeMap,
    sync::mpsc::{Receiver, Sender, SyncSender},
};

use chrono::Local;
use log::debug;
use timer::MessageTimer;

use super::{
    database::DatabaseManagerInput,
    statemap::{FilterOptions, StateMap, StateMapTrait},
    ExecsManagerInput,
};
use crate::concepts::{ActionFilter, Filter, LogEntry, Order, MFT};

#[derive(Clone)]
pub enum MatchManagerInput {
    Match(MFT),
    Unmatch(MFT),
    Order(Order, FilterOptions, SyncSender<MatchesMap>),
    EndOfStartup,
    Stop,
}

pub type MatchesMap = StateMap<Filter>;

pub fn matches_manager(
    match_rx: Receiver<MatchManagerInput>,
    match_tx: Sender<MatchManagerInput>,
    action_tx: Sender<ExecsManagerInput>,
    log_tx: SyncSender<DatabaseManagerInput>,
) {
    let mut matches: MatchesMap = BTreeMap::new();

    let timer = MessageTimer::new(match_tx);

    let mut startup = true;

    for mft in match_rx.iter() {
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
                        // retry_duration is always Some() after filter's setup
                        #[allow(clippy::unwrap_used)]
                        let guard = timer.schedule_with_delay(
                            mft.t - Local::now() + mft.o.retry_duration().unwrap(),
                            MatchManagerInput::Unmatch(mft.clone()),
                        );
                        guard.ignore();

                        matches.get_times(&mft) >= retry as usize
                    }
                };

                // Executing actions
                if exec {
                    // Delete matches only if storing them
                    if mft.o.retry().is_some() {
                        matches.rm_times(&mft);
                    }
                    mft.o.send_actions(&mft.m, mft.t, &action_tx);
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
