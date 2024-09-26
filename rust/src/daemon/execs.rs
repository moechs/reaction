use std::{
    collections::{BTreeMap, BTreeSet},
    process::Stdio,
    sync::mpsc::{Receiver, Sender, SyncSender},
};

use chrono::Local;
use log::{error, info};
use timer::MessageTimer;

use crate::{
    concepts::{Action, Config, Match, Time, MAT},
    utils::ThreadPool,
};

#[derive(Clone)]
pub enum ExecsManagerInput {
    Exec(MAT),
    ExecPending(MAT),
    Flush(MAT),
    Gimme(SyncSender<ExecsMap>),
    Stop,
}

type ExecsMap = BTreeMap<&'static Action, BTreeMap<Match, BTreeSet<Time>>>;

trait ExecsMapTrait {
    fn add(&mut self, mat: &MAT);
    fn rm(&mut self, mat: &MAT) -> bool;
    fn rm_times(&mut self, mat: &MAT) -> Option<BTreeSet<Time>>;
}
impl ExecsMapTrait for ExecsMap {
    fn add(&mut self, mat: &MAT) {
        let inner_map = self.entry(mat.a).or_default();
        let inner_set = inner_map.entry(mat.m.clone()).or_default();
        inner_set.insert(mat.t);
    }

    fn rm(&mut self, mat: &MAT) -> bool {
        let mut removed = false;
        if let Some(inner_map) = self.get_mut(&mat.a) {
            if let Some(inner_set) = inner_map.get_mut(&mat.m) {
                inner_set.remove(&mat.t);
                removed = true;
                if inner_set.is_empty() {
                    inner_map.remove(&mat.m);
                }
            }
            if inner_map.is_empty() {
                self.remove(&mat.a);
            }
        }
        removed
    }

    fn rm_times(&mut self, mat: &MAT) -> Option<BTreeSet<Time>> {
        let mut set = None;
        if let Some(inner_map) = self.get_mut(&mat.a) {
            set = inner_map.remove(&mat.m);
            if inner_map.is_empty() {
                self.remove(&mat.a);
            }
        }
        set
    }
}

pub fn execs_manager(
    config: &'static Config,
    exec_rx: Receiver<ExecsManagerInput>,
    exec_tx: Sender<ExecsManagerInput>,
) {
    // Initialize a ThreadPool only when concurrency hasn't been disabled
    let thread_pool = if config.concurrency() > 1 {
        Some(ThreadPool::new(config.concurrency()))
    } else {
        None
    };

    let exec_now = |mat: MAT| {
        let mut closure = {
            let action = mat.a;

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
            ExecsManagerInput::Flush(mat) => {
                if let Some(set) = execs.rm_times(&mat) {
                    for _ in set {
                        exec_now(mat.clone());
                    }
                }
            }
            #[allow(clippy::unwrap_used)] // propagating panics is ok
            ExecsManagerInput::Gimme(tx) => tx.send(execs.clone()).unwrap(),
            ExecsManagerInput::Stop => {
                for (action, inner_map) in execs {
                    if action.on_exit() {
                        for (match_, inner_set) in inner_map {
                            for _ in inner_set {
                                exec_now(MAT {
                                    m: match_.clone(),
                                    a: action,
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
