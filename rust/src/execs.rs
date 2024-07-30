use std::{
    collections::{BTreeMap, BTreeSet},
    process::Stdio,
    sync::{
        mpsc::{Receiver, Sender},
        Arc,
    },
};

use chrono::{DateTime, Local};
use log::{error, info};
use timer::MessageTimer;

use crate::{
    action::ActionName,
    config::Config,
    messages::{Match, MAT},
    utils::ThreadPool,
};

#[derive(Clone)]
pub enum ExecsManagerInput {
    Exec(MAT),
    ExecPending(MAT),
    #[allow(dead_code)]
    Flush(MAT),
    Stop,
}

type ExecsMap = BTreeMap<ActionName, BTreeMap<Match, BTreeSet<DateTime<Local>>>>;

trait ExecsMapTrait {
    fn add(&mut self, mft: &MAT);
    fn rm(&mut self, mft: &MAT);
}
impl ExecsMapTrait for ExecsMap {
    fn add(&mut self, mat: &MAT) {
        let inner_map = self.entry(mat.a.clone()).or_default();
        let inner_set = inner_map.entry(mat.m.clone()).or_default();
        inner_set.insert(mat.t);
    }

    fn rm(&mut self, mat: &MAT) {
        if let Some(inner_map) = self.get_mut(&mat.a) {
            if let Some(inner_set) = inner_map.get_mut(&mat.m) {
                inner_set.remove(&mat.t);
                if inner_set.is_empty() {
                    inner_map.remove(&mat.m);
                }
            }
            if inner_map.is_empty() {
                self.remove(&mat.a);
            }
        }
    }
}

pub fn execs_manager(
    config: Arc<Config>,
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
        // Retrieve needed information from config
        let filter = config.get_filter(&mat.a.stream, &mat.a.filter).unwrap();
        let action = filter.get_action(&mat.a.action).unwrap();
        let patterns = filter.patterns();

        // Construct command
        let mut command = action.exec(&mat.m, patterns);

        let mut closure = move || {
            info!("{}: run [{:?}]", &mat.a, command);
            if let Err(err) = command
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .stdout(Stdio::piped())
                .status()
            {
                error!("{}: run [{:?}], code {}", &mat.a, command, err);
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

    while let Ok(mat) = exec_rx.recv() {
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
                execs.rm(&mat);
                exec_now(mat);
            }
            #[allow(clippy::todo)]
            ExecsManagerInput::Flush(_mat) => todo!(),
            ExecsManagerInput::Stop => {
                for (action_name, inner_map) in execs {
                    if config
                        .get_filter(&action_name.stream, &action_name.filter)
                        .unwrap()
                        .get_action(&action_name.action)
                        .unwrap()
                        .on_exit()
                    {
                        for (match_, inner_set) in inner_map {
                            for _ in inner_set {
                                exec_now(MAT {
                                    m: match_.clone(),
                                    a: action_name.clone(),
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
