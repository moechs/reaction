use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    str::FromStr,
    sync::Arc,
};

use pingora_core::protocols::l4::socket::SocketAddr;
use reaction_plugin::{ActionConfig, ActionImpl, Exec, StreamConfig, Value};
use remoc::rch::mpsc;
use serde::{Deserialize, Serialize};

use crate::action::{IpSet, IpSets, ip_sets_from_names};

pub fn load_config(
    streams: Vec<StreamConfig>,
    actions: Vec<ActionConfig>,
    options: Vec<Value>,
) -> Result<(ComputedMainOptions, Vec<ComputedAction>, Vec<ActionImpl>), String> {
    if !streams.is_empty() {
        return Err("This plugin can't handle any stream type".into());
    }

    let options: MainOptions =
        serde_json::from_value(options.into()).map_err(|err| format!("{err}"))?;

    let domain_sets = {
        let mut vec = HashSet::new();
        for domain in options.domains.values() {
            match &domain.case {
                CaseOrCases::Case(case) => {
                    vec.insert(case.set.clone());
                }
                CaseOrCases::Cases(cases) => vec.extend(cases.iter().map(|case| case.set.clone())),
            }
        }
        vec
    };

    let ip_sets = ip_sets_from_names(&domain_sets);

    let computed_main_options = options.setup(&ip_sets)?;

    let mut computed_action_options = Vec::new();
    let mut action_sets = HashSet::new();
    let mut ret_actions = Vec::new();

    for ActionConfig {
        stream_name,
        filter_name,
        action_name,
        action_type,
        config,
        patterns,
    } in actions
    {
        if &action_type != "httpban" {
            return Err("This plugin can't handle other action types than httpban".into());
        }

        let name = format!("{stream_name}.{filter_name}.{action_name}");

        let options: ActionOptions = serde_json::from_value(config.into())
            .map_err(|err| format!("invalid options for action {name}: {err}"))?;

        if let Some(set) = &options.add {
            action_sets.insert(set.clone());
        }
        if let Some(set) = &options.remove {
            action_sets.insert(set.clone());
        }

        let (tx, rx) = remoc::rch::mpsc::channel(1);

        match options.setup(&patterns, &ip_sets, rx) {
            Ok(computed) => {
                computed_action_options.push(computed);
                ret_actions.push(ActionImpl { tx });
            }
            Err(err) => match err {
                Some(err) => Err(format!("invalid action {name}: {err}"))?,
                None => {
                    // That's fine, we will return an error later about the set mismatch between domains and actions
                }
            },
        }
    }

    // check that all sets referenced by domains or by actions are used by the each others.
    {
        let mut errors = Vec::new();
        errors.extend(domain_sets.difference(&action_sets).map(|set|
                format!("Set {set} is referenced by a domain, but is not referenced in any action, so it would stay empty forever and never match")
            ));
        errors.extend(action_sets.difference(&domain_sets).map(|set| format!("Set {set} is referenced by an action, but is not used in any domain, so it would never be actually used")));
        if !errors.is_empty() {
            return Err(errors.join("\n").into());
        }
    }

    Ok((computed_main_options, computed_action_options, ret_actions))
}

#[derive(Serialize, Deserialize)]
pub struct ActionOptions {
    /// The set we will add this IP to
    pub add: Option<String>,
    /// The set we will remove this IP from
    pub remove: Option<String>,
    /// The pattern name of the IP.
    /// Defaults to "ip"
    #[serde(default = "serde_ip")]
    pub pattern: String,
}
fn serde_ip() -> String {
    "ip".into()
}
impl ActionOptions {
    pub fn setup(
        self,
        patterns: &Vec<String>,
        ip_sets: &IpSets,
        rx: mpsc::Receiver<Exec>,
    ) -> Result<ComputedAction, Option<String>> {
        let (set_name, action) = if let Some(set) = self.add {
            if self.remove.is_none() {
                Ok((set, AddRemove::Add))
            } else {
                Err(Some("can't add AND remove in the same action".into()))
            }
        } else if let Some(set) = self.remove {
            Ok((set, AddRemove::Remove))
        } else {
            Err(Some("must either add or remove the IP from a set".into()))
        }?;

        // We error but we mark the error as skipable
        // The mismatch between action and domain sets will be catch later by the caller
        let set = ip_sets.get(&set_name).ok_or(None)?;

        let pattern = patterns
            .iter()
            .enumerate()
            .find(|p| &self.pattern == p.1)
            .ok_or_else(|| Some(format!("No pattern named {} in filter", self.pattern)))?
            .0;

        Ok(ComputedAction {
            action,
            pattern,
            set: set.clone(),
            rx,
        })
    }
}

#[cfg_attr(test, derive(Debug))]
pub struct ComputedAction {
    pub action: AddRemove,
    pub set: IpSet,
    pub pattern: usize,
    pub rx: mpsc::Receiver<Exec>,
}
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub enum AddRemove {
    Add,
    Remove,
}

#[cfg(test)]
impl PartialEq for ComputedAction {
    fn eq(&self, other: &Self) -> bool {
        self.action == other.action
            && self.pattern == other.pattern
            && self.set.blocking_read().name == other.set.blocking_read().name
    }
}
#[cfg(test)]
impl Eq for ComputedAction {}

#[derive(Serialize, Deserialize)]
pub struct MainOptions {
    // pub port: u16,
    pub backends: HashMap<String, Backend>,
    pub domains: HashMap<String, Domain>,
}

impl MainOptions {
    pub fn setup(self, ipsets: &IpSets) -> Result<ComputedMainOptions, String> {
        let backends = self
                .backends
                .into_iter()
                .map(|(k, b)| {
                    Ok((
                        k.clone(),
                        ComputedBackend {
                            name: k,
                            addr: SocketAddr::from_str(&b.host).map_err(|err| {
                                format!(
                                    "Invalid backend address: {err}. Format is either unix:PATH or IP:PORT"
                                )
                            })?,
                        }.into(),
                    ))
                })
                .collect::<Result<_, String>>()?;

        let mut domains = HashMap::new();

        for (key, domain) in self.domains {
            let (host, path) = key.split_once('/').unwrap_or_else(|| (&key, ""));

            let host_map: &mut HashMap<_, _> = domains.entry(host.into()).or_default();

            host_map.insert(path.into(), domain.setup(key, ipsets, &backends)?);
        }

        Ok(ComputedMainOptions {
            // port: self.port,
            backends,
            domains,
        })
    }
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct ComputedMainOptions {
    // pub port: u16,
    pub backends: ComputedBackends,
    pub domains: ComputedDomains,
}
pub type ComputedBackends = HashMap<String, Arc<ComputedBackend>>;
pub type ComputedDomains = HashMap<String, HashMap<String, ComputedDomain>>;

#[derive(Serialize, Deserialize)]
pub struct Backend {
    pub host: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ComputedBackend {
    pub name: String,
    pub addr: SocketAddr,
}

#[derive(Serialize, Deserialize)]
pub struct Domain {
    #[serde(default)]
    pub case: CaseOrCases,
    pub default: CaseOptions,
}

impl Domain {
    pub fn setup(
        self,
        name: String,
        ipsets: &IpSets,
        backends: &ComputedBackends,
    ) -> Result<ComputedDomain, String> {
        self._setup(name.clone(), ipsets, backends)
            .map_err(|err| format!("domain {name}: {err}"))
    }
    fn _setup(
        self,
        name: String,
        ipsets: &IpSets,
        backends: &ComputedBackends,
    ) -> Result<ComputedDomain, String> {
        fn get_ipset(ipsets: &IpSets, name: &str) -> Result<IpSet, String> {
            ipsets
                .get(name)
                .map(|ipset| ipset.clone())
                .ok_or_else(|| format!("No set {name} has been defined"))
        }
        Ok(ComputedDomain {
            cases: {
                match self.case {
                    CaseOrCases::Case(case) => {
                        let mut cases = Vec::with_capacity(1);
                        cases.push((get_ipset(ipsets, &case.set)?, case.options.setup(backends)?));
                        cases
                    }
                    CaseOrCases::Cases(user_cases) => {
                        let mut cases = Vec::with_capacity(user_cases.len());
                        let mut used_sets = HashSet::new();

                        for case in user_cases {
                            if used_sets.contains(&case.set) {
                                return Err(format!("set {} used as a condition twice", case.set));
                            }
                            cases.push((
                                get_ipset(ipsets, &case.set)?,
                                case.options.setup(backends)?,
                            ));
                            used_sets.insert(case.set);
                        }
                        cases
                    }
                }
            },
            default: self
                .default
                .setup(backends)
                .map_err(|err| format!("default case: {err}"))?,
            name,
        })
    }
}

#[derive(Debug)]
pub struct ComputedDomain {
    pub name: String,
    pub cases: Vec<(IpSet, ComputedCaseOptions)>,
    pub default: ComputedCaseOptions,
}

#[cfg(test)]
impl PartialEq for ComputedDomain {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.default == other.default
            && self.cases.len() == other.cases.len()
            && {
                self.cases
                    .iter()
                    .zip(other.cases.iter())
                    .all(|(selfcase, othercase)| {
                        selfcase.1 == othercase.1
                            && selfcase.0.blocking_read().name == othercase.0.blocking_read().name
                    })
            }
    }
}
#[cfg(test)]
impl Eq for ComputedDomain {}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum CaseOrCases {
    Case(Case),
    Cases(Vec<Case>),
}

impl Default for CaseOrCases {
    fn default() -> Self {
        Self::Cases(vec![])
    }
}

#[derive(Serialize, Deserialize)]
pub struct Case {
    pub set: String,
    #[serde(flatten)]
    options: CaseOptions,
}

#[derive(Serialize, Deserialize)]
pub struct CaseOptions {
    backend: Option<String>,
    code: Option<u16>,
    page: Option<String>,
    inline: Option<String>,
}

impl CaseOptions {
    pub fn setup(self, backends: &ComputedBackends) -> Result<ComputedCaseOptions, String> {
        match self {
            CaseOptions {
                backend: Some(backend),
                code: None,
                page: None,
                inline: None,
            } => Ok(ComputedCaseOptions::Backend(
                backends
                    .get(&backend)
                    .ok_or_else(|| format!("backend {backend} is not defined"))?
                    .clone(),
            )),
            CaseOptions {
                backend: None,
                code: Some(code),
                page: None,
                inline: Some(inline),
            } => Ok(ComputedCaseOptions::Static(StaticResponse {
                code,
                data: inline.into_bytes(),
            })),
            CaseOptions {
                backend: None,
                code: Some(code),
                page: Some(page),
                inline: None,
            } => Ok(ComputedCaseOptions::Static(StaticResponse {
                code,
                data: {
                    let mut data = Vec::new();

                    let mut file = File::open(&page)
                        .map_err(|err| format!("Cannot open page {page}: {err}"))?;

                    file.read_to_end(&mut data)
                        .map_err(|err| format!("Cannot read page: {page}: {err}"))?;

                    data
                },
            })),
            _ => Err(format!(
                "Invalid options: either `backend` must be specified, or `code` alongside one of `page` or `inline`"
            )),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ComputedCaseOptions {
    Backend(Arc<ComputedBackend>),
    Static(StaticResponse),
}

#[derive(Debug, PartialEq, Eq)]
pub struct StaticResponse {
    code: u16,
    data: Vec<u8>,
}

// TODO test config loading

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        io::Write,
        net::{Ipv4Addr, SocketAddr as StdInetSocketAddr, SocketAddrV4},
        os::unix::net::SocketAddr as StdUnixSocketAddr,
        sync::Arc,
    };

    use pingora_core::protocols::l4::socket::SocketAddr;
    use reaction_plugin::ActionConfig;
    use remoc::rch::mpsc;
    use serde_json::json;
    use tempfile::NamedTempFile;

    use crate::{
        action::DualStackSet,
        config::{
            AddRemove, ComputedAction, ComputedBackend, ComputedCaseOptions, ComputedDomain,
            ComputedMainOptions, StaticResponse, load_config,
        },
    };

    #[test]
    fn smol_config() {
        let config = json!({
          // "port": 2048,
          "backends": {
            "nginx": {
              "host": "unix:/run/nginx/nginx.sock",
            },
          },
          "domains": {
            "tokipona.ppom.me": {
              "case": {
                "set": "pubsites",
                "code": 401,
                "inline": "Nope!",
              },
              "default": {
                "backend": "nginx",
              },
            },
          },
        });

        let actions = vec![ActionConfig {
            stream_name: "s".into(),
            filter_name: "f".into(),
            action_name: "a_pubsites".into(),
            action_type: "httpban".into(),
            patterns: vec!["ip".into()],
            config: json!({
                "add": "pubsites",
            })
            .into(),
        }];

        let (computed_config, computed_actions, _) =
            load_config(vec![], actions, vec![config.into()]).unwrap();

        let ipsets = HashMap::from([("pubsites", DualStackSet::new("pubsites".into()))]);

        let backends = HashMap::from([(
            "nginx".into(),
            Arc::new(ComputedBackend {
                name: "nginx".into(),
                addr: SocketAddr::Unix(
                    StdUnixSocketAddr::from_pathname("/run/nginx/nginx.sock").unwrap(),
                ),
            }),
        )]);

        let domains = HashMap::from([(
            "tokipona.ppom.me".into(),
            HashMap::from([(
                "".into(),
                ComputedDomain {
                    name: "tokipona.ppom.me".into(),
                    cases: vec![(
                        ipsets.get("pubsites").unwrap().clone(),
                        ComputedCaseOptions::Backend(backends.get("iocaine").unwrap().clone()),
                    )],
                    default: ComputedCaseOptions::Backend(backends.get("nginx").unwrap().clone()),
                },
            )]),
        )]);

        let target_config = ComputedMainOptions {
            // port: 2048,
            backends,
            domains,
        };

        assert_eq!(computed_config, target_config);

        let target_actions = vec![
            ComputedAction {
                action: AddRemove::Add,
                set: ipsets.get("pubsites").unwrap().clone(),
                pattern: 0,
                rx: mpsc::channel(0).1,
            },
            ComputedAction {
                action: AddRemove::Remove,
                set: ipsets.get("pubapps").unwrap().clone(),
                pattern: 1,
                rx: mpsc::channel(0).1,
            },
            ComputedAction {
                action: AddRemove::Remove,
                set: ipsets.get("privapps").unwrap().clone(),
                pattern: 0,
                rx: mpsc::channel(0).1,
            },
        ];

        assert_eq!(computed_actions, target_actions);
    }

    #[test]
    fn big_config() {
        let html_data = "<html>
<head><title>No way!</title></head>
<body>You can't see this page</body>
</html>";

        let mut html_file = NamedTempFile::with_suffix(".html").unwrap();
        html_file.write_all(html_data.as_bytes()).unwrap();

        let html_path = html_file.into_temp_path();

        let config = json!({
          // "port": 2048,
          "backends": {
            "funkwhale": {
              "host": "127.0.0.1:8999",
            },
            "vault": {
              "host": "127.0.0.1:8998",
            },
            "iocaine": {
              "host": "unix:/run/iocaine/iocaine.sock",
            },
            "nginx": {
              "host": "unix:/run/nginx/nginx.sock",
            },
          },
          "domains": {
            "music.ppom.me": {
              "case": [
                {
                  "set": "pubapps",
                  "code": 404,
                  "page": *html_path,
                },
              ],
              "default": {
                "backend": "funkwhale",
              },
            },
            "ppom.me/vault": {
              "case": [
                {
                  "set": "privapps",
                  "code": 403,
                  "inline": "Unauthorized uwu",
                },
                {
                  "set": "pubapps",
                  "backend": "iocaine",
                },
              ],
              "default": {
                "backend": "vault",
              },
            },
            "ppom.me": {
              "default": {
                "backend": "nginx",
              },
            },
            "tokipona.ppom.me": {
              "case": {
                "set": "pubsites",
                "backend": "iocaine",
              },
              "default": {
                "backend": "nginx",
              },
            },
          },
        });

        let actions = vec![
            ActionConfig {
                stream_name: "s".into(),
                filter_name: "f".into(),
                action_name: "a_pubsites".into(),
                action_type: "httpban".into(),
                patterns: vec!["ip".into()],
                config: json!({
                    "add": "pubsites",
                })
                .into(),
            },
            ActionConfig {
                stream_name: "s".into(),
                filter_name: "f".into(),
                action_name: "a_pubapps".into(),
                action_type: "httpban".into(),
                patterns: vec!["abc".into(), "ip".into()],
                config: json!({
                    "remove": "pubapps",
                })
                .into(),
            },
            ActionConfig {
                stream_name: "s".into(),
                filter_name: "f".into(),
                action_name: "a_privapps".into(),
                action_type: "httpban".into(),
                patterns: vec!["ip".into()],
                config: json!({
                    "remove": "privapps",
                })
                .into(),
            },
        ];

        let (computed_config, computed_actions, _) =
            load_config(vec![], actions, vec![config.into()]).unwrap();

        let ipsets = HashMap::from([
            ("privapps", DualStackSet::new("privapps".into())),
            ("pubapps", DualStackSet::new("pubapps".into())),
            ("pubsites", DualStackSet::new("pubsites".into())),
        ]);

        let backends = HashMap::from([
            (
                "funkwhale".into(),
                Arc::new(ComputedBackend {
                    name: "funkwhale".into(),
                    addr: SocketAddr::Inet(StdInetSocketAddr::V4(SocketAddrV4::new(
                        Ipv4Addr::from_octets([127, 0, 0, 1]),
                        8999,
                    ))),
                }),
            ),
            (
                "vault".into(),
                Arc::new(ComputedBackend {
                    name: "vault".into(),
                    addr: SocketAddr::Inet(StdInetSocketAddr::V4(SocketAddrV4::new(
                        Ipv4Addr::from_octets([127, 0, 0, 1]),
                        8998,
                    ))),
                }),
            ),
            (
                "iocaine".into(),
                Arc::new(ComputedBackend {
                    name: "iocaine".into(),
                    addr: SocketAddr::Unix(
                        StdUnixSocketAddr::from_pathname("/run/iocaine/iocaine.sock").unwrap(),
                    ),
                }),
            ),
            (
                "nginx".into(),
                Arc::new(ComputedBackend {
                    name: "nginx".into(),
                    addr: SocketAddr::Unix(
                        StdUnixSocketAddr::from_pathname("/run/nginx/nginx.sock").unwrap(),
                    ),
                }),
            ),
        ]);

        let domains = HashMap::from([
            (
                "music.ppom.me".into(),
                HashMap::from([(
                    "".into(),
                    ComputedDomain {
                        name: "music.ppom.me".into(),
                        cases: vec![(
                            ipsets.get("pubapps").unwrap().clone(),
                            ComputedCaseOptions::Static(StaticResponse {
                                code: 404,
                                data: html_data.into(),
                            }),
                        )],
                        default: ComputedCaseOptions::Backend(
                            backends.get("funkwhale").unwrap().clone(),
                        ),
                    },
                )]),
            ),
            (
                "ppom.me".into(),
                HashMap::from([
                    (
                        "".into(),
                        ComputedDomain {
                            name: "ppom.me".into(),
                            cases: vec![],
                            default: ComputedCaseOptions::Backend(
                                backends.get("nginx").unwrap().clone(),
                            ),
                        },
                    ),
                    (
                        "/vault".into(),
                        ComputedDomain {
                            name: "ppom.me/vault".into(),
                            cases: vec![
                                (
                                    ipsets.get("privapps").unwrap().clone(),
                                    ComputedCaseOptions::Static(StaticResponse {
                                        code: 403,
                                        data: "Unauthorized uwu".into(),
                                    }),
                                ),
                                (
                                    ipsets.get("pubapps").unwrap().clone(),
                                    ComputedCaseOptions::Backend(
                                        backends.get("iocaine").unwrap().clone(),
                                    ),
                                ),
                            ],
                            default: ComputedCaseOptions::Backend(
                                backends.get("vault").unwrap().clone(),
                            ),
                        },
                    ),
                ]),
            ),
            (
                "tokipona.ppom.me".into(),
                HashMap::from([(
                    "".into(),
                    ComputedDomain {
                        name: "tokipona.ppom.me".into(),
                        cases: vec![(
                            ipsets.get("pubsites").unwrap().clone(),
                            ComputedCaseOptions::Backend(backends.get("iocaine").unwrap().clone()),
                        )],
                        default: ComputedCaseOptions::Backend(
                            backends.get("nginx").unwrap().clone(),
                        ),
                    },
                )]),
            ),
        ]);

        let target_config = ComputedMainOptions {
            // port: 2048,
            backends,
            domains,
        };

        assert_eq!(computed_config, target_config);

        let target_actions = vec![
            ComputedAction {
                action: AddRemove::Add,
                set: ipsets.get("pubsites").unwrap().clone(),
                pattern: 0,
                rx: mpsc::channel(0).1,
            },
            ComputedAction {
                action: AddRemove::Remove,
                set: ipsets.get("pubapps").unwrap().clone(),
                pattern: 1,
                rx: mpsc::channel(0).1,
            },
            ComputedAction {
                action: AddRemove::Remove,
                set: ipsets.get("privapps").unwrap().clone(),
                pattern: 0,
                rx: mpsc::channel(0).1,
            },
        ];

        assert_eq!(computed_actions, target_actions);
    }
}
