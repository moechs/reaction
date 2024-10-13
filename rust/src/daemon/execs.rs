use std::{collections::BTreeMap, process::Stdio, sync::Arc};

use chrono::Local;
use tokio::{
    sync::{mpsc, watch, Semaphore},
    time,
};
use tracing::{error, info};

use crate::concepts::{Action, ActionFilter, Config, LogEntry, Order, MAT};

use super::{
    database::DatabaseManagerInput,
    socket::SocketOrder,
    statemap::{StateMap, StateMapTrait},
};

pub type ExecsMap = StateMap<Action>;

pub async fn execs_manager(
    config: &'static Config,
    mut exec_rx: mpsc::Receiver<MAT>,
    mut socket_order_rx: mpsc::Receiver<SocketOrder<ExecsMap>>,
    log_tx: mpsc::Sender<DatabaseManagerInput>,
    mut stop: watch::Receiver<bool>,
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
                #[allow(clippy::unwrap_used)] // We know the semaphore is not closed
                Some(semaphore) => Some(semaphore.acquire_owned().await.unwrap()),
                None => None,
            };

            // Construct command
            let mut command = action.exec(&mat.m);

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
    };

    let mut execs: ExecsMap = BTreeMap::new();

    let (pendings_tx, mut pendings_rx) = mpsc::channel(1);

    loop {
        tokio::select! {
            _ = stop.changed() => break,
            Some(mat) = exec_rx.recv() => {
                let now = Local::now();
                if mat.t < now {
                    exec_now(mat);
                } else {
                     execs.add(&mat);
                    {
                        let mat = mat.clone();
                        let pendings_tx = pendings_tx.clone();
                        tokio::spawn(async move {
                            let dur = mat.t - now;
                            time::sleep(dur.to_std().expect("Duration is bigger than what's supported. Did you put an enormous after duration?")).await;
                            #[allow(clippy::unwrap_used)] // propagating panics is ok
                            pendings_tx
                                .send(mat.clone())
                                .await
                                .unwrap();
                        });
                    }
                }
            }
            Some(mat) = pendings_rx.recv() => {
                if execs.rm(&mat) {
                    exec_now(mat);
                }
            }
            Some((order, options, tx)) = socket_order_rx.recv() => {
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
            else => break
        }
    }

    for (action, inner_map) in &mut execs {
        if action.on_exit() {
            for (match_, inner_set) in inner_map {
                for _ in inner_set.iter() {
                    exec_now(MAT {
                        m: match_.clone(),
                        o: action,
                        t: Local::now(),
                    });
                }
            }
        }
    }
}
