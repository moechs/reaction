use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

use futures::{SinkExt, StreamExt};
use reaction_plugin::shutdown::ShutdownToken;
use regex::Regex;
use tokio::{fs, net::UnixListener};
use tokio_util::{
    bytes::Bytes,
    codec::{Framed, LengthDelimitedCodec},
};
use tracing::{error, warn};

use crate::{
    concepts::{Config, Filter, Pattern, Patterns, Stream, now},
    protocol::{ClientRequest, ClientStatus, DaemonResponse, Order},
};

use super::filter::FilterManager;

pub type SharedState = HashMap<&'static Stream, HashMap<&'static Filter, FilterManager>>;

async fn open_socket(path: &Path) -> Result<UnixListener, String> {
    macro_rules! err_str {
        ($expression:expr) => {
            $expression.map_err(|err| err.to_string())
        };
    }
    // First create all directories to the file
    let dir = path
        .parent()
        .ok_or(format!("socket {path:?} has no parent directory"))?;
    err_str!(fs::create_dir_all(dir).await)?;
    // Test if file exists
    match fs::metadata(&path).await {
        Ok(meta) => {
            if meta.file_type().is_dir() {
                Err(format!("socket {path:?} is already a directory"))
            } else {
                warn!("socket {path:?} already exists: is the daemon already running? deleting.");
                err_str!(fs::remove_file(path).await)
            }
        }
        Err(err) => err_str!(match err.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(err),
        }),
    }?;
    // Open socket
    err_str!(UnixListener::bind(path))
}

async fn handle_trigger_order(
    stream_name: Option<String>,
    filter_name: Option<String>,
    patterns: BTreeMap<Arc<Pattern>, String>,
    shared_state: &SharedState,
) -> DaemonResponse {
    // Check names existence
    let (stream_name, filter_name) = match (stream_name, filter_name) {
        (Some(s), Some(p)) => (s, p),
        _ => {
            return DaemonResponse::Err(
                "trigger must target a filter, e.g. `reaction trigger mystream.myfilter ...`"
                    .into(),
            );
        }
    };

    // Check patterns existence
    if patterns.is_empty() {
        return DaemonResponse::Err(
            "trigger must specify patterns, e.g. `reaction trigger ... ip=1.2.3.4`".into(),
        );
    }

    // Check stream existence
    let filters = match shared_state
        .iter()
        .find(|(stream, _)| stream_name == stream.name)
    {
        Some((_, filters)) => filters,
        None => {
            return DaemonResponse::Err(format!("stream {stream_name} doesn't exist"));
        }
    };

    // Check filter existence
    let filter_manager = match filters
        .iter()
        .find(|(filter, _)| filter_name == filter.name)
    {
        Some((_, filter)) => filter,
        None => {
            return DaemonResponse::Err(format!(
                "filter {stream_name}.{filter_name} doesn't exist"
            ));
        }
    };

    match filter_manager.handle_trigger(patterns, now()).await {
        Ok(()) => DaemonResponse::Ok(()),
        Err(err) => DaemonResponse::Err(err),
    }
}

async fn handle_show_or_flush_order(
    stream_name: Option<String>,
    filter_name: Option<String>,
    patterns: BTreeMap<Arc<Pattern>, Regex>,
    order: Order,
    shared_state: &SharedState,
) -> DaemonResponse {
    let now = now();
    let iter = shared_state
        .iter()
        // stream filtering
        .filter(|(stream, _)| {
            stream_name.is_none() || stream_name.clone().is_some_and(|name| name == stream.name)
        });
    let mut cs = ClientStatus::new();
    for (stream, filter_manager) in iter {
        let iter = filter_manager
            .iter()
            // filter filtering
            .filter(|(filter, _)| {
                filter_name.is_none() || filter_name.clone().is_some_and(|name| name == filter.name)
            })
            // pattern filtering
            .filter(|(filter, _)| {
                patterns
                    .iter()
                    .all(|(pattern, _)| filter.patterns.get(pattern).is_some())
            });
        let mut inner_map = BTreeMap::new();
        for (filter, manager) in iter {
            inner_map.insert(
                filter.name.to_owned(),
                manager.handle_order(&patterns, order, now).await,
            );
        }
        cs.insert(stream.name.to_owned(), inner_map);
    }
    DaemonResponse::Order(cs)
}

fn split_stream_name(stream_filter: Option<String>) -> (Option<String>, Option<String>) {
    match stream_filter {
        Some(sf) => match sf.split_once(".") {
            Some((s, f)) => (Some(s.to_string()), Some(f.to_string())),
            None => (Some(sf), None),
        },
        None => (None, None),
    }
}

fn compute_patterns(
    patterns: Vec<(String, String)>,
    config_patterns: &Patterns,
) -> Result<BTreeMap<Arc<Pattern>, String>, String> {
    patterns
        .into_iter()
        .map(|(name, reg)| {
            // lookup pattern in config.patterns
            config_patterns
                .iter()
                // retrieve or Err
                .find(|(pattern_name, _)| &name == *pattern_name)
                .ok_or_else(|| format!("pattern '{name}' doesn't exist"))
                .map(|(_, pattern)| (pattern.clone(), reg))
        })
        .collect()
}

async fn answer_order(
    config: &'static Config,
    shared_state: &SharedState,
    options: ClientRequest,
) -> DaemonResponse {
    // Compute options
    let (stream_name, filter_name) = split_stream_name(options.stream_filter);

    let patterns = match compute_patterns(options.patterns, &config.patterns) {
        Ok(p) => p,
        Err(err) => return DaemonResponse::Err(err),
    };

    if let Order::Trigger = options.order {
        handle_trigger_order(stream_name, filter_name, patterns, shared_state).await
    } else {
        let patterns = match patterns
            .into_iter()
            .map(|(pattern, reg)| match Regex::new(&reg) {
                Ok(reg) => Ok((pattern, reg)),
                Err(err) => Err(format!(
                    "pattern '{}' regex doesn't compile: {err}",
                    pattern.name
                )),
            })
            .collect::<Result<BTreeMap<Arc<Pattern>, Regex>, String>>()
        {
            Ok(p) => p,
            Err(err) => return DaemonResponse::Err(err),
        };

        handle_show_or_flush_order(
            stream_name,
            filter_name,
            patterns,
            options.order,
            shared_state,
        )
        .await
    }
}

macro_rules! or_next {
    ($msg:expr, $expression:expr) => {
        match $expression {
            Ok(x) => x,
            Err(err) => {
                error!("failed to answer client: {}, {}", $msg, err);
                continue;
            }
        }
    };
}

pub struct Socket {
    path: PathBuf,
    socket: UnixListener,
}

impl Socket {
    pub async fn open(socket: PathBuf) -> Result<Self, String> {
        Ok(Socket {
            socket: open_socket(&socket)
                .await
                .map_err(|err| format!("while creating communication socket: {err}"))?,
            path: socket,
        })
    }

    pub fn manager(
        self,
        config: &'static Config,
        shared_state: SharedState,
        shutdown: ShutdownToken,
    ) {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.wait() => break,
                    try_conn = self.socket.accept() => {
                        match try_conn {
                            Ok((conn, _)) => {
                                let mut transport = Framed::new(conn, LengthDelimitedCodec::new());
                                // Decode
                                let received = transport.next().await;
                                let encoded_request = match received {
                                    Some(r) => or_next!("while reading request", r),
                                    None => {
                                        error!("failed to answer client: client sent no request");
                                        continue;
                                    }
                                };
                                let request = or_next!(
                                    "failed to decode request",
                                    serde_json::from_slice(&encoded_request)
                                    );
                                // Process
                                let response = answer_order(config, &shared_state, request).await;
                                // Encode
                                let encoded_response =
                                    or_next!("failed to serialize response", serde_json::to_string::<DaemonResponse>(&response));
                                or_next!(
                                    "failed to send response:",
                                    transport.send(Bytes::from(encoded_response)).await
                                    );
                            }
                            Err(err) => error!("failed to open connection from cli: {err}"),
                        }
                    }
                }
            }

            if let Err(err) = fs::remove_file(self.path).await {
                error!("failed to remove socket: {}", err);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use assert_fs::TempDir;
    use tokio::fs::File;

    use crate::concepts::{Pattern, Patterns};

    use super::{compute_patterns, open_socket, split_stream_name};

    #[tokio::test]
    async fn open_socket_empty_dir() {
        let tmp_dir = TempDir::new().unwrap();
        let res = open_socket(&tmp_dir).await;
        assert_eq!(
            res.err(),
            Some(format!(
                "socket {:?} is already a directory",
                tmp_dir.path()
            ))
        );
    }

    #[tokio::test]
    async fn open_socket_in_empty_dir() {
        let tmp_dir = TempDir::new().unwrap();
        let path = tmp_dir.join("socket.sock");
        let res = open_socket(&path).await;
        assert!(res.is_ok());
        assert!(path.try_exists().is_ok_and(|b| b));
    }

    #[tokio::test]
    async fn open_socket_in_creatable_dir() {
        let tmp_dir = TempDir::new().unwrap();
        let path = tmp_dir.join("subdir").join("subsubdir").join("socket.sock");
        let res = open_socket(&path).await;
        assert!(res.is_ok());
        assert!(path.try_exists().is_ok_and(|b| b));
    }

    #[tokio::test]
    async fn open_socket_in_noncreatable_dir() {
        // skip this test if we're root, as we may have the rights
        // to create this directory
        #[allow(unsafe_code)]
        let euid = unsafe { nix::libc::geteuid() };
        if euid != 0 {
            let path = PathBuf::from("/zblerg/zblorg/zblurg/socket.sock");
            let res = open_socket(&path).await;
            assert!(res.is_ok());
            assert!(path.try_exists().is_ok_and(|b| b));
        }
    }

    #[tokio::test]
    async fn open_socket_on_replacable_file() {
        let tmp_dir = TempDir::new().unwrap();
        let path = tmp_dir.join("socket.sock");
        File::create(&path).await.unwrap();
        // This should write a warning to stderr, but we can't easily test that
        let res = open_socket(&path).await;
        assert!(res.is_ok());
        assert!(path.try_exists().is_ok_and(|b| b));
    }

    #[test]
    fn split_stream_name_test() {
        assert_eq!(split_stream_name(None), (None, None));
        assert_eq!(
            split_stream_name(Some("abcde".into())),
            (Some("abcde".into()), None)
        );
        assert_eq!(
            split_stream_name(Some("a.b".into())),
            (Some("a".into()), Some("b".into()))
        );
        assert_eq!(
            split_stream_name(Some("abcde.abc".into())),
            (Some("abcde".into()), Some("abc".into()))
        );
    }

    fn sample_patterns() -> Patterns {
        Patterns::from([
            (
                "boubou".into(),
                Pattern {
                    regex: "abcd[efg]".into(),
                    ..Default::default()
                }
                .into(),
            ),
            (
                "bibi".into(),
                Pattern {
                    regex: "abcde[fgh]".into(),
                    ..Default::default()
                }
                .into(),
            ),
            (
                "baba".into(),
                Pattern {
                    regex: "ba(ba)+".into(),
                    ..Default::default()
                }
                .into(),
            ),
        ])
    }

    #[test]
    fn compute_patterns_test() {
        let config_patterns = sample_patterns();
        assert_eq!(
            compute_patterns(vec![("baba".into(), "one".into())], &config_patterns),
            Ok(BTreeMap::from([(
                config_patterns["baba"].to_owned(),
                "one".to_string()
            )])),
        );
        assert_eq!(
            compute_patterns(
                vec![("baba".into(), "one".into()), ("bibi".into(), "two".into())],
                &config_patterns,
            ),
            Ok(BTreeMap::from([
                (config_patterns["baba"].to_owned(), "one".to_string()),
                (config_patterns["bibi"].to_owned(), "two".into())
            ])),
        );
        assert_eq!(
            compute_patterns(
                vec![
                    ("baba".into(), "one".into()),
                    ("boubou".into(), "two".into()),
                    ("bibi".into(), "three".into())
                ],
                &config_patterns,
            ),
            Ok(BTreeMap::from([
                (config_patterns["baba"].to_owned(), "one".to_string()),
                (config_patterns["boubou"].to_owned(), "two".into()),
                (config_patterns["bibi"].to_owned(), "three".into())
            ])),
        );

        assert!(compute_patterns(vec![("bobo".into(), "one".into())], &config_patterns).is_err());
        assert!(
            compute_patterns(
                vec![("baba".into(), "one".into()), ("bobo".into(), "two".into())],
                &config_patterns
            )
            .is_err()
        );
    }
}
