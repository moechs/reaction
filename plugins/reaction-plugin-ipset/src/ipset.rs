use std::{collections::BTreeMap, fmt::Display, net::Ipv4Addr, process::Command, thread};

use ipset::{
    Session,
    types::{HashNet, NetDataType, Parse},
};
use tokio::sync::{mpsc, oneshot};

#[derive(PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
pub enum Version {
    IPv4,
    IPv6,
}
impl Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Version::IPv4 => "IPv4",
            Version::IPv6 => "IPv6",
        })
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct CreateSet {
    pub name: String,
    pub version: Version,
    pub timeout: Option<u32>,
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct SetChain {
    pub set: String,
    pub version: Version,
    pub chain: String,
    pub target: String,
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum Order {
    CreateSet(CreateSet),
    DestroySet(String),
    InsertSet(SetChain),
    RemoveSet(SetChain),
    Add(String, String),
    Del(String, String),
}

#[derive(Clone)]
pub struct IpSet {
    tx: mpsc::Sender<OrderType>,
}

impl Default for IpSet {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel(1);
        thread::spawn(move || IPsetManager::default().serve(rx));
        Self { tx }
    }
}

impl IpSet {
    pub async fn order(&mut self, order: Order) -> Result<(), IpSetError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send((order, tx))
            .await
            .map_err(|err| IpSetError::Thread(format!("ipset thread has quit: {err}")))?;
        rx.await
            .map_err(|err| IpSetError::Thread(format!("ipset thread didn't respond: {err}")))?
            .map_err(IpSetError::IpSet)
    }
}

pub enum IpSetError {
    Thread(String),
    IpSet(()),
}
impl Display for IpSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                IpSetError::Thread(err) => err,
                IpSetError::IpSet(()) => "ipset error",
            }
        )
    }
}
impl From<IpSetError> for String {
    fn from(value: IpSetError) -> Self {
        match value {
            IpSetError::Thread(err) => err,
            IpSetError::IpSet(()) => "ipset error".to_string(),
        }
    }
}

pub type OrderType = (Order, oneshot::Sender<Result<(), ()>>);

pub type Set = Session<HashNet>;

#[derive(Default)]
struct IPsetManager {
    // IPset sessions
    sessions: BTreeMap<String, Set>,
}

impl IPsetManager {
    fn serve(mut self, mut rx: mpsc::Receiver<OrderType>) {
        loop {
            match rx.blocking_recv() {
                None => break,
                Some((order, response)) => {
                    let result = self.handle_order(order);
                    let _ = response.send(result);
                }
            }
        }
    }

    fn handle_order(&mut self, order: Order) -> Result<(), ()> {
        match order {
            Order::CreateSet(CreateSet {
                name,
                version,
                timeout,
            }) => {
                eprintln!("INFO creating  {version} set {name}");
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
                    .map_err(|err| eprintln!("ERROR Could not create set {name}: {err}"))?;

                self.sessions.insert(name, session);
            }
            Order::DestroySet(set) => {
                eprintln!("INFO destroying set {set}");
                let mut session = self
                    .sessions
                    .remove(&set)
                    .unwrap_or_else(|| Session::new(set.clone()));

                session
                    .destroy()
                    .map_err(|err| eprintln!("ERROR Could not destroy set {set}: {err}"))?;
            }

            Order::InsertSet(options) => self.insert_remove_set(options, true)?,
            Order::RemoveSet(options) => self.insert_remove_set(options, false)?,

            Order::Add(set, ip) => self.insert_remove_ip(set, ip, true)?,
            Order::Del(set, ip) => self.insert_remove_ip(set, ip, false)?,
        };
        Ok(())
    }

    fn insert_remove_ip(&mut self, set: String, ip: String, insert: bool) -> Result<(), ()> {
        self._insert_remove_ip(set, ip, insert)
            .map_err(|err| eprintln!("ERROR {err}"))
    }
    fn _insert_remove_ip(&mut self, set: String, ip: String, insert: bool) -> Result<(), String> {
        let session = self.sessions.get_mut(&set).ok_or(format!(
            "No set handled by this plugin with this name: {set}. This likely is a bug."
        ))?;

        let mut net_data = NetDataType::new(Ipv4Addr::LOCALHOST, 0);
        net_data
            .parse(&ip)
            .map_err(|err| format!("`{ip}` is not recognized as an IP: {err}"))?;

        if insert {
            session.add(net_data, &[])
        } else {
            session.del(net_data)
        }
        .map_err(|err| format!("Could not add `{ip}` to set {set}: {err}"))?;

        Ok(())
    }

    fn insert_remove_set(&self, options: SetChain, insert: bool) -> Result<(), ()> {
        self._insert_remove_set(options, insert)
            .map_err(|err| eprintln!("ERROR {err}"))
    }
    fn _insert_remove_set(&self, options: SetChain, insert: bool) -> Result<(), String> {
        let SetChain {
            set,
            version,
            chain,
            target,
        } = options;

        let (verb, verbing, from) = if insert {
            ("insert", "inserting", "in")
        } else {
            ("remove", "removing", "from")
        };

        eprintln!("INFO {verbing} {version} set {set} {from} chain {chain}");

        let command = match version {
            Version::IPv4 => "iptables",
            Version::IPv6 => "ip6tables",
        };

        let mut child = Command::new(command)
            .args([
                "-w",
                if insert { "-I" } else { "-D" },
                &chain,
                "-m",
                "set",
                "--match-set",
                &set,
                "src",
                "-j",
                &target,
            ])
            .spawn()
            .map_err(|err| format!("Could not {verb} ipset {set} {from} chain {chain}: Could not execute {command}: {err}"))?;

        let exit = child
            .wait()
            .map_err(|err| format!("Could not {verb} ipset {set} {from} chain {chain}: {err}"))?;

        if exit.success() {
            Ok(())
        } else {
            Err(format!(
                "Could not {verb} ipset: exit code {}",
                exit.code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string())
            ))
        }
    }
}
