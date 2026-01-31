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
    IpSet(String),
}
impl Display for IpSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                IpSetError::Thread(err) => err,
                IpSetError::IpSet(err) => err,
            }
        )
    }
}
impl From<IpSetError> for String {
    fn from(value: IpSetError) -> Self {
        match value {
            IpSetError::Thread(err) => err,
            IpSetError::IpSet(err) => err,
        }
    }
}

pub type OrderType = (Order, oneshot::Sender<Result<(), String>>);

struct Set {
    session: Session<HashNet>,
    version: Version,
}

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

    fn handle_order(&mut self, order: Order) -> Result<(), String> {
        match order {
            Order::CreateSet(CreateSet {
                name,
                version,
                timeout,
            }) => {
                eprintln!("INFO creating {version} set {name}");
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
                    eprintln!("INFO destroying {} set {set}", session.version);
                    session
                        .session
                        .destroy()
                        .map_err(|err| format!("Could not destroy set {set}: {err}"))?;
                }
            }

            Order::InsertSet(options) => self.insert_remove_set(options, true)?,
            Order::RemoveSet(options) => self.insert_remove_set(options, false)?,

            Order::Add(set, ip) => self.insert_remove_ip(set, ip, true)?,
            Order::Del(set, ip) => self.insert_remove_ip(set, ip, false)?,
        };
        Ok(())
    }

    fn insert_remove_ip(&mut self, set: String, ip: String, insert: bool) -> Result<(), String> {
        let session = self.sessions.get_mut(&set).ok_or(format!(
            "No set handled by us with this name: {set}. This likely is a bug."
        ))?;

        let mut net_data = NetDataType::new(Ipv4Addr::LOCALHOST, 0);
        net_data
            .parse(&ip)
            .map_err(|err| format!("`{ip}` is not recognized as an IP: {err}"))?;

        if insert {
            session.session.add(net_data, &[])
        } else {
            session.session.del(net_data)
        }
        .map_err(|err| format!("Could not add `{ip}` to set {set}: {err}"))?;

        Ok(())
    }

    fn insert_remove_set(&self, options: SetChain, insert: bool) -> Result<(), String> {
        let SetChain {
            set,
            chain,
            target: action,
        } = options;

        let version = self
            .sessions
            .get(&set)
            .ok_or(format!("No set managed by us with this name: {set}"))?
            .version;

        if insert {
            eprintln!("INFO inserting {version} set {set} in chain {chain}");
        } else {
            eprintln!("INFO removing {version} set {set} from chain {chain}");
        }

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
                &action,
            ])
            .spawn()
            .map_err(|err| format!("Could not insert ipset {set} in chain {chain}: {err}"))?;

        let exit = child
            .wait()
            .map_err(|err| format!("Could not insert ipset: {err}"))?;

        if exit.success() {
            Ok(())
        } else {
            Err(format!(
                "Could not insert ipset: exit code {}",
                exit.code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string())
            ))
        }
    }
}
