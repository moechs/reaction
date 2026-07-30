use std::{collections::BTreeMap, fs::File, io::Read};

use serde::{Deserialize, Serialize};

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

pub struct ComputedMainOptions {
    pub backends: BTreeMap<String, ComputedBackend>,
    pub domains: BTreeMap<String, ComputedDomain>,
}

#[derive(Serialize, Deserialize)]
pub struct Backend {
    #[serde(skip)]
    pub name: String,

    pub host: String,
}

pub struct ComputedBackend {
    pub name: String,
    pub host: String,
}

#[derive(Serialize, Deserialize)]
pub struct Domain {
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
                    CaseOrCases::None(_) => (),
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

pub struct ComputedDomain {
    pub name: String,
    pub cases: BTreeMap<String, ComputedCaseOptions>,
    pub default: ComputedCaseOptions,
}

#[derive(Serialize, Deserialize)]
pub enum CaseOrCases {
    None(()),
    Case(Case),
    Cases(Vec<Case>),
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
    code: Option<String>,
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

pub enum ComputedCaseOptions {
    Backend(BackendSpecifier),
    Static(StaticResponse),
}

// TODO parse the path
pub struct BackendSpecifier {
    backend: String,
}

pub struct StaticResponse {
    code: String,
    data: Vec<u8>,
}

// TODO test config loading
