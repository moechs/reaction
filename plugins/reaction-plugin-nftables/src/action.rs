use std::{
    borrow::Cow,
    collections::HashSet,
    fmt::{Debug, Display},
};

use nftables::{
    batch::Batch,
    expr::{Expression, NamedExpression, Payload, PayloadField, Prefix},
    schema::{Element, NfListObject, Rule, SetFlag, SetType, SetTypeValue},
    stmt::{Match, Operator, Statement},
    types::{NfFamily, NfHook},
};
use reaction_plugin::{Exec, shutdown::ShutdownToken, time::parse_duration};
use remoc::rch::mpsc as remocMpsc;
use serde::{Deserialize, Serialize};

use crate::{helpers::Version, nft::NftClient};

#[derive(Default, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum IpVersion {
    #[default]
    #[serde(rename = "ip")]
    Ip,
    #[serde(rename = "ipv4")]
    Ipv4,
    #[serde(rename = "ipv6")]
    Ipv6,
}
impl Debug for IpVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                IpVersion::Ipv4 => "ipv4",
                IpVersion::Ipv6 => "ipv6",
                IpVersion::Ip => "ip",
            }
        )
    }
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub enum AddDel {
    #[default]
    #[serde(rename = "add")]
    Add,
    #[serde(rename = "delete", alias = "del")]
    Delete,
}

/// User-facing action options
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionOptions {
    /// The set that should be used by this action
    pub set: String,
    /// The pattern name of the IP.
    /// Defaults to "ip"
    #[serde(default = "serde_ip")]
    pub pattern: String,
    #[serde(skip)]
    ip_index: usize,
    // Whether the action is to "add" or "del" the ip from the set
    #[serde(default)]
    action: AddDel,

    #[serde(flatten)]
    pub set_options: SetOptions,
}

fn serde_ip() -> String {
    "ip".into()
}

impl ActionOptions {
    pub fn set_ip_index(&mut self, patterns: Vec<String>) -> Result<(), ()> {
        self.ip_index = patterns
            .into_iter()
            .enumerate()
            .find(|(_, name)| name == &self.pattern)
            .ok_or(())?
            .0;
        Ok(())
    }
}

/// Merged set options
#[derive(Default, Clone, Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct SetOptions {
    /// The IP type.
    /// Defaults to `46`.
    /// If `ipv4`: creates an IPv4 set with this name
    /// If `ipv6`: creates an IPv6 set with this name
    /// If `ip`: creates an IPv4 set with its name suffixed by 'v4' AND an IPv6 set with its name suffixed by 'v6'
    /// *Merged set-wise*.
    #[serde(default)]
    version: Option<IpVersion>,
    /// Chains where the IP set should be inserted.
    /// Defaults to `["input", "forward"]`
    /// *Merged set-wise*.
    #[serde(default)]
    hooks: Option<Vec<RHook>>,
    // Optional timeout, letting linux/netfilter handle set removal instead of reaction
    // Note that `reaction show` and `reaction flush` won't work if set instead of an `after` action
    // Same syntax as after and retryperiod in reaction.
    /// *Merged set-wise*.
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<String>,
    #[serde(skip)]
    timeout_u32: Option<u32>,
    // Target that iptables should use when the IP is encountered.
    // Defaults to DROP, but can also be ACCEPT, RETURN or any user-defined chain
    /// *Merged set-wise*.
    #[serde(default)]
    target: Option<RStatement>,
}

impl SetOptions {
    pub fn merge(&mut self, options: &SetOptions) -> Result<(), String> {
        // merge two Option<T> and fail if there is conflict
        fn inner_merge<T: Eq + Clone + std::fmt::Debug>(
            a: &mut Option<T>,
            b: &Option<T>,
            name: &str,
        ) -> Result<(), String> {
            match (&a, &b) {
                (Some(aa), Some(bb)) => {
                    if aa != bb {
                        return Err(format!(
                            "Conflicting options for {name}: `{aa:?}` and `{bb:?}`"
                        ));
                    }
                }
                (None, Some(_)) => {
                    *a = b.clone();
                }
                _ => (),
            };
            Ok(())
        }

        inner_merge(&mut self.version, &options.version, "version")?;
        inner_merge(&mut self.timeout, &options.timeout, "timeout")?;
        inner_merge(&mut self.hooks, &options.hooks, "chains")?;
        inner_merge(&mut self.target, &options.target, "target")?;

        if let Some(timeout) = &self.timeout {
            let duration = parse_duration(timeout)
                .map_err(|err| format!("failed to parse timeout: {}", err))?
                .as_secs();
            if duration > u32::MAX as u64 {
                return Err(format!(
                    "timeout is limited to {} seconds (approx {} days)",
                    u32::MAX,
                    49_000
                ));
            }
            self.timeout_u32 = Some(duration as u32);
        }

        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Eq, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RHook {
    Ingress,
    Prerouting,
    Forward,
    Input,
    Output,
    Postrouting,
    Egress,
}

impl RHook {
    pub fn as_str(&self) -> &'static str {
        match self {
            RHook::Ingress => "ingress",
            RHook::Prerouting => "prerouting",
            RHook::Forward => "forward",
            RHook::Input => "input",
            RHook::Output => "output",
            RHook::Postrouting => "postrouting",
            RHook::Egress => "egress",
        }
    }
}

impl Display for RHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<&RHook> for NfHook {
    fn from(value: &RHook) -> Self {
        match value {
            RHook::Ingress => Self::Ingress,
            RHook::Prerouting => Self::Prerouting,
            RHook::Forward => Self::Forward,
            RHook::Input => Self::Input,
            RHook::Output => Self::Output,
            RHook::Postrouting => Self::Postrouting,
            RHook::Egress => Self::Egress,
        }
    }
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Eq, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RStatement {
    Accept,
    Drop,
    Continue,
    Return,
}

pub struct Set {
    pub sets: SetNames,
    pub hooks: Vec<RHook>,
    pub timeout: Option<u32>,
    pub target: RStatement,
}

impl Set {
    pub fn from(name: String, options: SetOptions) -> Self {
        Self {
            sets: SetNames::new(name, options.version),
            timeout: options.timeout_u32,
            target: options.target.unwrap_or(RStatement::Drop),
            hooks: options.hooks.unwrap_or(vec![RHook::Input, RHook::Forward]),
        }
    }

    pub fn init<'a>(&self, batch: &mut Batch<'a>) -> Result<(), String> {
        for (set, version) in [
            (&self.sets.ipv4, Version::IPv4),
            (&self.sets.ipv6, Version::IPv6),
        ] {
            if let Some(set) = set {
                let family = NfFamily::INet;
                let table = Cow::from("reaction");

                // create set
                batch.add(NfListObject::<'a>::Set(Box::new(nftables::schema::Set::<
                    'a,
                > {
                    family,
                    table: table.to_owned(),
                    name: Cow::Owned(set.to_owned()),
                    set_type: SetTypeValue::Single(match &version {
                        Version::IPv4 => SetType::Ipv4Addr,
                        Version::IPv6 => SetType::Ipv6Addr,
                    }),
                    flags: Some({
                        let mut flags = HashSet::from([SetFlag::Interval]);
                        if self.timeout.is_some() {
                            flags.insert(SetFlag::Timeout);
                        }
                        flags
                    }),
                    timeout: self.timeout.clone(),
                    ..Default::default()
                })));
                // Add set rule in the chains
                for hook in &self.hooks {
                    batch.add(NfListObject::Rule(Rule {
                        family,
                        table: table.to_owned(),
                        chain: Cow::from(hook.to_string()),
                        expr: Cow::Owned(vec![
                            // If source address in set
                            Statement::Match(Match {
                                op: Operator::EQ,
                                left: Expression::Named(NamedExpression::Payload(
                                    Payload::PayloadField(PayloadField {
                                        protocol: Cow::Borrowed(match version {
                                            Version::IPv4 => "ip",
                                            Version::IPv6 => "ip6",
                                        }),
                                        field: Cow::Borrowed("saddr"),
                                    }),
                                )),
                                right: Expression::String(format!("@{}", set).into()),
                            }),
                            // Do this action
                            match self.target {
                                RStatement::Accept => Statement::Accept(None),
                                RStatement::Drop => Statement::Drop(None),
                                RStatement::Continue => Statement::Continue(None),
                                RStatement::Return => Statement::Return(None),
                            },
                        ]),
                        ..Default::default()
                    }));
                }
            }
        }
        Ok(())
    }
}

pub struct SetNames {
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
}

impl SetNames {
    pub fn new(name: String, version: Option<IpVersion>) -> Self {
        Self {
            ipv4: match version {
                Some(IpVersion::Ipv4) => Some(name.clone()),
                Some(IpVersion::Ipv6) => None,
                None | Some(IpVersion::Ip) => Some(format!("{}v4", name)),
            },
            ipv6: match version {
                Some(IpVersion::Ipv4) => None,
                Some(IpVersion::Ipv6) => Some(name),
                None | Some(IpVersion::Ip) => Some(format!("{}v6", name)),
            },
        }
    }
}

pub struct Action {
    nft: NftClient,
    rx: remocMpsc::Receiver<Exec>,
    shutdown: ShutdownToken,
    sets: SetNames,
    // index of pattern ip in match vec
    ip_index: usize,
    action: AddDel,
}

impl Action {
    pub fn new(
        nft: NftClient,
        shutdown: ShutdownToken,
        rx: remocMpsc::Receiver<Exec>,
        options: ActionOptions,
    ) -> Result<Self, String> {
        Ok(Action {
            nft,
            rx,
            shutdown,
            sets: SetNames::new(options.set, options.set_options.version),
            ip_index: options.ip_index,
            action: options.action,
        })
    }

    pub async fn serve(mut self) {
        loop {
            let event = tokio::select! {
                exec = self.rx.recv() => Some(exec),
                _ = self.shutdown.wait() => None,
            };
            match event {
                // shutdown asked
                None => break,
                // channel closed
                Some(Ok(None)) => break,
                // error from channel
                Some(Err(err)) => {
                    eprintln!("ERROR {err}");
                    break;
                }
                // ok
                Some(Ok(Some(exec))) => {
                    if let Err(err) = self.handle_exec(exec).await {
                        eprintln!("ERROR {err}");
                        // break;
                    }
                }
            }
        }
        eprintln!("DEBUG Asking for shutdown");
        self.shutdown.ask_shutdown();
    }

    async fn handle_exec(&mut self, mut exec: Exec) -> Result<(), String> {
        // safeguard against Vec::remove's panic
        if exec.match_.len() <= self.ip_index {
            return Err(format!(
                "match received from reaction is smaller than expected. looking for index {} but size is {}. this is a bug!",
                self.ip_index,
                exec.match_.len()
            ));
        }
        let ip = exec.match_.remove(self.ip_index);
        // select set
        let set = match (&self.sets.ipv4, &self.sets.ipv6) {
            (None, None) => return Err(format!("action is neither IPv4 nor IPv6, this is a bug!")),
            (None, Some(set)) => set,
            (Some(set), None) => set,
            (Some(set4), Some(set6)) => {
                if ip.contains(':') {
                    set6
                } else {
                    set4
                }
            }
        };
        // add/remove ip to set
        let element = NfListObject::Element(Element {
            family: NfFamily::INet,
            table: Cow::from("reaction"),
            name: Cow::from(set),
            elem: match ip.split_once("/") {
                // IP has no range, we give it as-is
                None => Cow::from(vec![Expression::String(Cow::from(ip.clone()))]),

                // IP has a range, we must split it ourselves
                Some((ip, prefix)) => match u32::from_str_radix(prefix, 10) {
                    Ok(len) => {
                        Cow::from(vec![Expression::Named(NamedExpression::Prefix(Prefix {
                            addr: Box::new(Expression::String(ip.into())),
                            len,
                        }))])
                    }
                    Err(err) => {
                        return Err(format!(
                            "CIDR part of {ip}/{prefix} can't be converted to u32, this is a bug! error: {err}"
                        ));
                    }
                },
            },
        });

        let mut batch = Batch::new();
        match self.action {
            AddDel::Add => batch.add(element),
            AddDel::Delete => batch.delete(element),
        };
        match self.nft.send(batch).await {
            Ok(ok) => {
                eprintln!("DEBUG action ok {:?} {ip}: {ok}", self.action);
                Ok(())
            }
            Err(err) => Err(format!("action ko {:?} {ip}: {err}", self.action)),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::action::{IpVersion, RHook, RStatement, SetOptions};

    #[tokio::test]
    async fn set_options_merge() {
        let s1 = SetOptions {
            version: None,
            hooks: None,
            timeout: None,
            timeout_u32: None,
            target: None,
        };
        let s2 = SetOptions {
            version: Some(IpVersion::Ipv4),
            hooks: Some(vec![RHook::Input]),
            timeout: Some("3h".into()),
            timeout_u32: Some(3 * 3600),
            target: Some(RStatement::Drop),
        };
        assert_ne!(s1, s2);
        assert_eq!(s1, SetOptions::default());

        {
            // s2 can be merged in s1
            let mut s1 = s1.clone();
            assert!(s1.merge(&s2).is_ok());
            assert_eq!(s1, s2);
        }

        {
            // s1 can be merged in s2
            let mut s2 = s2.clone();
            assert!(s2.merge(&s1).is_ok());
        }

        {
            // s1 can be merged in itself
            let mut s3 = s1.clone();
            assert!(s3.merge(&s1).is_ok());
            assert_eq!(s1, s3);
        }

        {
            // s2 can be merged in itself
            let mut s3 = s2.clone();
            assert!(s3.merge(&s2).is_ok());
            assert_eq!(s2, s3);
        }

        for s3 in [
            SetOptions {
                version: Some(IpVersion::Ipv6),
                ..Default::default()
            },
            SetOptions {
                hooks: Some(vec![RHook::Output]),
                ..Default::default()
            },
            SetOptions {
                timeout: Some("30min".into()),
                ..Default::default()
            },
            SetOptions {
                target: Some(RStatement::Continue),
                ..Default::default()
            },
        ] {
            // none with some is ok
            assert!(s3.clone().merge(&s1).is_ok(), "s3: {s3:?}");
            assert!(s1.clone().merge(&s3).is_ok(), "s3: {s3:?}");
            // different some is ko
            assert!(s3.clone().merge(&s2).is_err(), "s3: {s3:?}");
            assert!(s2.clone().merge(&s3).is_err(), "s3: {s3:?}");
        }
    }
}
