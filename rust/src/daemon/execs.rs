use std::{collections::BTreeMap, process::Stdio, sync::Arc};

use chrono::Local;
use log::{error, info};
use tokio::{
    sync::{mpsc, oneshot, Semaphore},
    time,
};

use crate::concepts::{Action, ActionFilter, Config, LogEntry, Order, MAT};

use super::{
    database::DatabaseManagerInput,
    statemap::{FilterOptions, StateMap, StateMapTrait},
};

#[derive(Debug)]
pub enum ExecsManagerInput {
    Exec(MAT),
    ExecPending(MAT),
    Order(Order, FilterOptions, oneshot::Sender<ExecsMap>),
    Stop,
}

type ExecsMap = StateMap<Action>;

pub async fn execs_manager(
    config: &'static Config,
    mut exec_rx: mpsc::Receiver<ExecsManagerInput>,
    exec_tx: mpsc::Sender<ExecsManagerInput>,
    log_tx: mpsc::Sender<DatabaseManagerInput>,
) {
    // FIXME replace with TryStreamExt::try_for_each_concurrent?
    let semaphore = if config.concurrency() > 0 {
        Some(Arc::new(Semaphore::new(config.concurrency())))
    } else {
        None
    };

    let exec_now = |mat: MAT| {
        let semaphore = semaphore.clone();
        tokio::spawn(async move {
            let action = mat.o;

            // Wait for semaphore's permission, if it is Some
            let _permit = match semaphore {
                Some(semaphore) => Some(semaphore.acquire_owned().await.unwrap()),
                None => None,
            };

            // Construct command
            let mut command = action.exec(&mat.m);

            info!("{}: run [{:?}]", &action, command);
            if let Err(err) = command
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .stdout(Stdio::piped())
                .status()
                .await
            {
                error!("{}: run [{:?}], code {}", &action, command, err);
            }
        });
    };

    let mut execs: ExecsMap = BTreeMap::new();

    while let Some(mat) = exec_rx.recv().await {
        match mat {
            ExecsManagerInput::Exec(mat) => {
                let now = Local::now();
                if mat.t.lt(&now) {
                    exec_now(mat);
                } else {
                    execs.add(&mat);
                    {
                        let mat = mat.clone();
                        let exec_tx = exec_tx.clone();
                        tokio::spawn(async move {
                            let dur = mat.t - Local::now();
                            time::sleep(dur.to_std().unwrap()).await;
                            #[allow(clippy::unwrap_used)] // propagating panics is ok
                            exec_tx
                                .send(ExecsManagerInput::ExecPending(mat.clone()))
                                .await
                                .unwrap();
                        });
                    }
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
                                .await
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
}
