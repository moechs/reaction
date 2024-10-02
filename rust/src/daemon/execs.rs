use std::{
    collections::BTreeMap,
    process::Stdio,
    sync::mpsc::{Receiver, Sender, SyncSender},
};

use chrono::Local;
use log::{error, info};
use timer::MessageTimer;

use crate::{
    concepts::{Action, ActionFilter, Config, LogEntry, Order, MAT},
    utils::ThreadPool,
};

use super::{
    database::DatabaseManagerInput,
    statemap::{FilterOptions, StateMap, StateMapTrait},
};

#[derive(Clone)]
pub enum ExecsManagerInput {
    Exec(MAT),
    ExecPending(MAT),
    Order(Order, FilterOptions, SyncSender<ExecsMap>),
    Stop,
}

type ExecsMap = StateMap<Action>;

pub fn execs_manager(
    config: &'static Config,
    exec_rx: Receiver<ExecsManagerInput>,
    exec_tx: Sender<ExecsManagerInput>,
    log_tx: SyncSender<DatabaseManagerInput>,
) {
    // Initialize a ThreadPool only when concurrency hasn't been disabled
    let thread_pool = if config.concurrency() > 1 {
        Some(ThreadPool::new(config.concurrency()))
    } else {
        None
    };

    let exec_now = |mat: MAT| {
        let mut closure = {
            let action = mat.o;

            // Construct command
            let mut command = action.exec(&mat.m);

            move || {
                info!("{}: run [{:?}]", &action, command);
                if let Err(err) = command
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .stdout(Stdio::piped())
                    .status()
                {
                    error!("{}: run [{:?}], code {}", &action, command, err);
                }
            }
        };

        // Execute command either in the ThreadPool or directly
        match &thread_pool {
            Some(thread_pool) => {
                thread_pool.execute(closure);
            }
            None => closure(),
        }
    };

    let mut execs: ExecsMap = BTreeMap::new();

    let timer = MessageTimer::new(exec_tx);

    for mat in exec_rx.iter() {
        match mat {
            ExecsManagerInput::Exec(mat) => {
                let now = Local::now();
                if mat.t.lt(&now) {
                    exec_now(mat);
                } else {
                    execs.add(&mat);
                    let guard = timer
                        .schedule_with_date(mat.t, ExecsManagerInput::ExecPending(mat.clone()));
                    guard.ignore();
                }
            }
            ExecsManagerInput::ExecPending(mat) => {
                if execs.rm(&mat) {
                    exec_now(mat);
                }
            }
            ExecsManagerInput::Order(order, options, tx) => {
                let filtered = execs.filtered(options);

                if let Order::Flush = order {
                    let now = Local::now();
                    // filter the state_map according to provided options
                    for (action, inner_map) in &filtered {
                        // get filter (required for LogEntry, FIXME optimize this)
                        let filter = {
                            let name = action.full_name();
                            #[allow(clippy::unwrap_used)]
                            // We're pretty confident our action has a filter
                            config
                                .get_filter(&(name.0.to_string(), name.1.to_string()))
                                .unwrap()
                        };

                        for match_ in inner_map.keys() {
                            let mat = MAT {
                                m: match_.clone(),
                                o: action,
                                t: now,
                            };
                            // delete them from state and execute them
                            if let Some(set) = execs.rm_times(&mat) {
                                for _ in set {
                                    exec_now(mat.clone());
                                }
                            }
                            #[allow(clippy::unwrap_used)] // propagating panics is ok
                            log_tx
                                .send(DatabaseManagerInput::Flush(LogEntry {
                                    exec: false,
                                    m: mat.m,
                                    f: filter,
                                    t: mat.t,
                                }))
                                .unwrap();
                        }
                    }
                }
                #[allow(clippy::unwrap_used)] // propagating panics is ok
                tx.send(filtered).unwrap();
            }
            ExecsManagerInput::Stop => {
                for (action, inner_map) in execs {
                    if action.on_exit() {
                        for (match_, inner_set) in inner_map {
                            for _ in inner_set {
                                exec_now(MAT {
                                    m: match_.clone(),
                                    o: action,
                                    t: Local::now(),
                                });
                            }
                        }
                    }
                }
                break;
            }
        }
    }

    if let Some(thread_pool) = thread_pool {
        thread_pool.join();
    }
}
