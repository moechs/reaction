use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    ops::Index,
    sync::{Arc, RwLock},
};

use reaction_plugin::Exec;
use remoc::rch::mpsc;
use serde::{Deserialize, Serialize};

use crate::action::{IpSets, JointSets};

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
    ) -> Result<ComputedActionOptions, Option<String>> {
        let set_name = if let Some(set) = self.add {
            if self.remove.is_none() {
                Ok(set)
            } else {
                Err(Some("can't add AND remove in the same action".into()))
            }
        } else if let Some(set) = self.remove {
            Ok(set)
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

        Ok(ComputedActionOptions {
            action: AddRemove::Remove,
            pattern,
            set: set.clone(),
            rx,
        })
    }
}
pub struct ComputedActionOptions {
    pub action: AddRemove,
    pub set: Arc<RwLock<JointSets>>,
    pub pattern: usize,
    pub rx: mpsc::Receiver<Exec>,
}
pub enum AddRemove {
    Add,
    Remove,
}

#[derive(Serialize, Deserialize)]
pub struct MainOptions {
    pub backends: BTreeMap<String, Backend>,
    pub domains: BTreeMap<String, Domain>,
}

impl MainOptions {
    pub fn setup(self) -> Result<ComputedMainOptions, String> {
        Ok(ComputedMainOptions {
            backends: self
                .backends
                .into_iter()
                .map(|(k, b)| {
                    (
                        k.clone(),
                        ComputedBackend {
                            name: k,
                            host: b.host,
                        },
                    )
                })
                .collect(),
            // TODO check that backend exist
            domains: self
                .domains
                .into_iter()
                .map(|(name, domain)| Ok((name.clone(), domain.setup(name)?)))
                .collect::<Result<_, String>>()?,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ComputedMainOptions {
    pub backends: BTreeMap<String, ComputedBackend>,
    pub domains: BTreeMap<String, ComputedDomain>,
}

#[derive(Serialize, Deserialize)]
pub struct Backend {
    pub host: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ComputedBackend {
    pub name: String,
    pub host: String,
}

#[derive(Serialize, Deserialize)]
pub struct Domain {
    #[serde(default)]
    pub case: CaseOrCases,
    pub default: CaseOptions,
}

impl Domain {
    pub fn setup(self, name: String) -> Result<ComputedDomain, String> {
        self._setup(name.clone())
            .map_err(|err| format!("domain {name}: {err}"))
    }
    fn _setup(self, name: String) -> Result<ComputedDomain, String> {
        Ok(ComputedDomain {
            cases: {
                let mut cases = BTreeMap::default();
                match self.case {
                    // CaseOrCases::None => (),
                    CaseOrCases::Case(case) => {
                        cases.insert(case.set, case.options.setup()?);
                    }
                    CaseOrCases::Cases(user_cases) => {
                        for case in user_cases {
                            if cases.contains_key(&case.set) {
                                return Err(format!("set {} used as a condition twice", case.set));
                            }
                            cases.insert(case.set, case.options.setup()?);
                        }
                    }
                }
                cases
            },
            default: self
                .default
                .setup()
                .map_err(|err| format!("default case: {err}"))?,
            name,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ComputedDomain {
    pub name: String,
    pub cases: BTreeMap<String, ComputedCaseOptions>,
    pub default: ComputedCaseOptions,
}

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
    set: String,
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
    pub fn setup(self) -> Result<ComputedCaseOptions, String> {
        match self {
            CaseOptions {
                backend: Some(backend),
                code: None,
                page: None,
                inline: None,
            } => Ok(ComputedCaseOptions::Backend(BackendSpecifier { backend })),
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
    Backend(BackendSpecifier),
    Static(StaticResponse),
}

// TODO parse the path
#[derive(Debug, PartialEq, Eq)]
pub struct BackendSpecifier {
    backend: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct StaticResponse {
    code: u16,
    data: Vec<u8>,
}

// TODO test config loading

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, io::Write};

    use serde_json::json;
    use tempfile::NamedTempFile;

    use crate::config::{
        BackendSpecifier, ComputedBackend, ComputedCaseOptions, ComputedDomain,
        ComputedMainOptions, MainOptions, StaticResponse,
    };

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
          "backends": {
            "funkwhale": {
              "host": "http://localhost:8999",
            },
            "vault": {
              "host": "http://localhost:8998",
            },
            "iocaine": {
              "host": "unix:///run/iocaine/iocaine.sock",
            },
            "nginx": {
              "host": "unix:///run/nginx/nginx.sock",
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

        let config: MainOptions = serde_json::from_value(config).unwrap();
        let computed_config = config.setup().unwrap();
        let target_config = ComputedMainOptions {
            backends: BTreeMap::from([
                (
                    "funkwhale".into(),
                    ComputedBackend {
                        name: "funkwhale".into(),
                        host: "http://localhost:8999".into(),
                    },
                ),
                (
                    "vault".into(),
                    ComputedBackend {
                        name: "vault".into(),
                        host: "http://localhost:8998".into(),
                    },
                ),
                (
                    "iocaine".into(),
                    ComputedBackend {
                        name: "iocaine".into(),
                        host: "unix:///run/iocaine/iocaine.sock".into(),
                    },
                ),
                (
                    "nginx".into(),
                    ComputedBackend {
                        name: "nginx".into(),
                        host: "unix:///run/nginx/nginx.sock".into(),
                    },
                ),
            ]),
            domains: BTreeMap::from([
                (
                    "music.ppom.me".into(),
                    ComputedDomain {
                        name: "music.ppom.me".into(),
                        cases: BTreeMap::from([(
                            "pubapps".into(),
                            ComputedCaseOptions::Static(StaticResponse {
                                code: 404,
                                data: html_data.into(),
                            }),
                        )]),
                        default: ComputedCaseOptions::Backend(BackendSpecifier {
                            backend: "funkwhale".into(),
                        }),
                    },
                ),
                (
                    "ppom.me/vault".into(),
                    ComputedDomain {
                        name: "ppom.me/vault".into(),
                        cases: BTreeMap::from([
                            (
                                "privapps".into(),
                                ComputedCaseOptions::Static(StaticResponse {
                                    code: 403,
                                    data: "Unauthorized uwu".into(),
                                }),
                            ),
                            (
                                "pubapps".into(),
                                ComputedCaseOptions::Backend(BackendSpecifier {
                                    backend: "iocaine".into(),
                                }),
                            ),
                        ]),
                        default: ComputedCaseOptions::Backend(BackendSpecifier {
                            backend: "vault".into(),
                        }),
                    },
                ),
                (
                    "ppom.me".into(),
                    ComputedDomain {
                        name: "ppom.me".into(),
                        cases: BTreeMap::default(),
                        default: ComputedCaseOptions::Backend(BackendSpecifier {
                            backend: "nginx".into(),
                        }),
                    },
                ),
                (
                    "tokipona.ppom.me".into(),
                    ComputedDomain {
                        name: "tokipona.ppom.me".into(),
                        cases: BTreeMap::from([(
                            "pubsites".into(),
                            ComputedCaseOptions::Backend(BackendSpecifier {
                                backend: "iocaine".into(),
                            }),
                        )]),
                        default: ComputedCaseOptions::Backend(BackendSpecifier {
                            backend: "nginx".into(),
                        }),
                    },
                ),
            ]),
        };

        assert_eq!(computed_config, target_config);
    }
}
