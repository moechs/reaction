use std::{collections::BTreeMap, process::Command, thread};

use ipset::{
    Session,
    types::{Error, HashNet},
};
use tokio::sync::mpsc;

#[derive(PartialEq, Eq)]
pub enum Version {
    IPv4,
    IPv6,
}

pub struct SetOptions {
    name: String,
    version: Version,
    timeout: Option<u32>,
}

pub struct SetChain {
    set: String,
    chain: String,
    action: String,
}

pub enum Order {
    CreateSet(SetOptions),
    DestroySet(String),
    InsertSet(SetChain),
    RemoveSet(SetChain),
}

pub fn ipset_thread() -> Result<mpsc::Sender<Order>, String> {
    let (tx, rx) = mpsc::channel(1);
    thread::spawn(move || IPsetManager::default().serve(rx));
    Ok(tx)
}

struct Set {
    session: Session<HashNet>,
    version: Version,
}

#[derive(Default)]
struct IPsetManager {
    sessions: BTreeMap<String, Set>,
}

impl IPsetManager {
    fn serve(&mut self, mut rx: mpsc::Receiver<Order>) {
        loop {
            match rx.blocking_recv() {
                None => break,
                Some(order) => {
                    let result = self.handle_order(order);
                }
            }
        }
    }

    fn handle_order(&mut self, order: Order) -> Result<(), String> {
        match order {
            Order::CreateSet(SetOptions {
                name,
                version,
                timeout,
            }) => {
                let mut session: Session<HashNet> = Session::new(name.clone());
                session
                    .create(|builder| {
                        let builder = if let Some(timeout) = timeout {
                            builder.with_timeout(timeout)?
                        } else {
                            builder
                        };
                        builder.with_ipv6(version == Version::IPv6)?.build()
                    })
                    .map_err(|err| format!("Could not create set {name}: {err}"))?;

                self.sessions.insert(name, Set { session, version });
            }
            Order::DestroySet(set) => {
                if let Some(mut session) = self.sessions.remove(&set) {
                    session
                        .session
                        .destroy()
                        .map_err(|err| format!("Could not destroy set {set}: {err}"))?;
                }
            }
            Order::InsertSet(SetChain { set, chain, action }) => {
                let child = Command::new("iptables")
                    .args([
                        "-w",
                        "-I",
                        &chain,
                        "-m",
                        "set",
                        "--match-set",
                        &set,
                        "src",
                        "-j",
                        &action,
                    ])
                    .spawn()
                    .map_err(|err| {
                        format!("Could not insert ipset {set} in chain {chain}: {err}")
                    })?;
                match child.wait() {
                    Ok(exit) => {
                        if !exit.success() {
                            return Err(format!("Could not insert ipset"));
                        }
                    }
                    Err(_) => todo!(),
                };
            }
            Order::RemoveSet(options) => {}
        };
        Ok(())
    }
}
