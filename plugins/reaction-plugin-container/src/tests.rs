//! Plugin-level tests. The Docker scenarios run against an in-process mock of
//! the Docker Engine API served on a unix socket (see [`mock`]).

use std::time::{Duration, SystemTime};

use reaction_plugin::{ActionConfig, Line, PluginInfo, StreamConfig, StreamImpl, Value};
use serde_json::json;

use crate::plugin::Plugin;

fn stream(name: &str, stream_type: &str, options: serde_json::Value) -> StreamConfig {
    StreamConfig {
        stream_name: name.into(),
        stream_type: stream_type.into(),
        config: options.into(),
    }
}

#[tokio::test]
async fn manifest() {
    let manifest = Plugin::default().manifest().await.unwrap();
    assert!(manifest.streams.contains("docker"));
    assert!(manifest.actions.is_empty());
}

#[tokio::test]
async fn config_errors() {
    let ok_stream = || stream("s", "docker", json!({"match": {"names": ["web"]}}));

    // valid
    assert!(
        Plugin::default()
            .load_config(vec![ok_stream()], vec![], vec![])
            .await
            .is_ok()
    );
    // unknown stream type
    assert!(
        Plugin::default()
            .load_config(vec![stream("s", "podman", json!({}))], vec![], vec![])
            .await
            .is_err()
    );
    // invalid stream options
    assert!(
        Plugin::default()
            .load_config(vec![stream("s", "docker", json!({}))], vec![], vec![])
            .await
            .is_err()
    );
    // actions are not provided
    let action = ActionConfig {
        stream_name: "s".into(),
        filter_name: "f".into(),
        action_name: "a".into(),
        action_type: "docker".into(),
        config: Value::Null,
        patterns: vec![],
    };
    assert!(
        Plugin::default()
            .load_config(vec![ok_stream()], vec![action], vec![])
            .await
            .is_err()
    );
    // plugin options
    for bad in [
        json!({"unknown": 1}),
        json!({"reconnect_delay": "soon"}),
        json!({"reconnect_delay": "0s"}),
        json!({"docker": {"host": "ssh://x"}}),
        json!({"docker": {"nope": 1}}),
    ] {
        assert!(
            Plugin::default()
                .load_config(vec![ok_stream()], vec![], vec![bad.clone().into()])
                .await
                .is_err(),
            "should fail: {bad}"
        );
    }
    // options split across files are merged
    assert!(
        Plugin::default()
            .load_config(
                vec![ok_stream()],
                vec![],
                vec![
                    json!({"reconnect_delay": "1s"}).into(),
                    json!({"docker": {"host": "unix:///x.sock"}}).into()
                ]
            )
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn start_fails_when_docker_is_unreachable() {
    let dir = tempfile::tempdir().unwrap();
    let host = format!("unix://{}/missing.sock", dir.path().display());
    let mut plugin = Plugin::default();
    plugin
        .load_config(
            vec![stream("s", "docker", json!({"match": {"names": ["web"]}}))],
            vec![],
            vec![json!({"docker": {"host": host}}).into()],
        )
        .await
        .unwrap();
    assert!(plugin.start().await.is_err());
}

#[tokio::test]
async fn wait_for_runtime_starts_anyway() {
    let dir = tempfile::tempdir().unwrap();
    let host = format!("unix://{}/missing.sock", dir.path().display());
    let mut plugin = Plugin::default();
    plugin
        .load_config(
            vec![stream("s", "docker", json!({"match": {"names": ["web"]}}))],
            vec![],
            vec![json!({"docker": {"host": host}, "wait_for_runtime": true, "reconnect_delay": "50ms"}).into()],
        )
        .await
        .unwrap();
    plugin.start().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), plugin.close())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn wait_for_runtime_baseline_starts_when_connected() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("docker.sock");
    let docker = mock::MockDocker::serve(&socket);
    docker.set_ready(false);

    let web = docker.add("web", &[], true);
    // Written after `start()` but before the daemon becomes reachable.
    let during_startup = SystemTime::now() + Duration::from_millis(500);
    docker.log(&web, 1, mock::ts_at(during_startup), "during startup");

    let mut plugin = Plugin::default();
    let (mut streams, _) = plugin
        .load_config(
            vec![stream("s", "docker", json!({"match": {"names": ["web"]}}))],
            vec![],
            vec![json!({
                "docker": {"host": format!("unix://{}", socket.display())},
                "wait_for_runtime": true,
                "reconnect_delay": "50ms",
            })
            .into()],
        )
        .await
        .unwrap();
    plugin.start().await.unwrap();
    let mut s = streams.pop().unwrap();

    tokio::time::sleep(Duration::from_millis(800)).await;
    docker.set_ready(true);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_silent(&mut s).await;

    let after_connect = SystemTime::now() + Duration::from_secs(2);
    docker.log(&web, 1, mock::ts_at(after_connect), "after connect");
    assert_eq!(recv(&mut s).await.0, "after connect");
    plugin.close().await.unwrap();
}

async fn recv(stream: &mut StreamImpl) -> Line {
    tokio::time::timeout(Duration::from_secs(5), stream.stream.recv())
        .await
        .expect("timeout waiting for a line")
        .unwrap()
        .expect("stream closed")
}

async fn assert_silent(stream: &mut StreamImpl) {
    if let Ok(line) = tokio::time::timeout(Duration::from_millis(300), stream.stream.recv()).await {
        panic!("unexpected line: {:?}", line.unwrap().map(|l| l.0));
    }
}

#[tokio::test]
async fn docker_follow_discover_restart() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("docker.sock");
    let docker = mock::MockDocker::serve(&socket);

    let web = docker.add("web", &[("app", "shop")], true);
    docker.log(&web, 1, mock::ts(-3600), "too old");
    docker.log(&web, 1, mock::ts(1), "hello 1.2.3.4");
    docker.log(&web, 2, mock::ts(2), "an error");
    let other = docker.add("other", &[("app", "blog")], true);
    docker.log(&other, 1, mock::ts(1), "not followed");

    let mut plugin = Plugin::default();
    let (mut streams, _) = plugin
        .load_config(
            vec![
                stream(
                    "shop",
                    "docker",
                    json!({"match": {"labels": ["app=shop"]}, "prefix": "{name}/{source} "}),
                ),
                stream(
                    "shop_stderr",
                    "docker",
                    json!({"match": {"names": ["web"]}, "sources": ["stderr"], "timestamp": "runtime"}),
                ),
            ],
            vec![],
            vec![json!({
                "docker": {"host": format!("unix://{}", socket.display())},
                "reconnect_delay": "50ms",
            })
            .into()],
        )
        .await
        .unwrap();
    plugin.start().await.unwrap();
    let mut errs = streams.pop().unwrap();
    let mut shop = streams.pop().unwrap();

    // Snapshot: existing lines newer than startup, prefix rendered.
    assert_eq!(recv(&mut shop).await.0, "web/stdout hello 1.2.3.4");
    assert_eq!(recv(&mut shop).await.0, "web/stderr an error");
    // Runtime timestamps are forwarded as-is.
    let (line, time) = recv(&mut errs).await;
    assert_eq!(line, "an error");
    assert_eq!(time, mock::ts_duration(2));

    // Live line.
    docker.log(&web, 1, mock::ts(3), "live");
    assert_eq!(recv(&mut shop).await.0, "web/stdout live");

    // The connection drops while the container is still running:
    // the follower reconnects without duplicating lines.
    docker.drop_logs(&web);
    docker.log(&web, 1, mock::ts(4), "after reconnect");
    assert_eq!(recv(&mut shop).await.0, "web/stdout after reconnect");

    // A new matching container is discovered through events.
    let api = docker.add("api", &[("app", "shop")], true);
    docker.log(&api, 1, mock::ts(5), "api up");
    docker.event("start", &api);
    assert_eq!(recv(&mut shop).await.0, "api/stdout api up");

    // A non-matching container is ignored.
    let db = docker.add("db", &[], true);
    docker.log(&db, 1, mock::ts(5), "db up");
    docker.event("start", &db);

    // Rename: the prefix follows the new name.
    docker.rename(&api, "api2", &[("app", "shop")]);
    docker.event("rename", &api);
    tokio::time::sleep(Duration::from_millis(200)).await;
    docker.log(&api, 1, mock::ts(6), "renamed");
    assert_eq!(recv(&mut shop).await.0, "api2/stdout renamed");

    // Rename out of the selector: the container is no longer followed.
    docker.rename(&api, "api3", &[("app", "other")]);
    docker.event("rename", &api);
    tokio::time::sleep(Duration::from_millis(200)).await;
    docker.log(&api, 1, mock::ts(7), "not followed anymore");

    // Stop and restart web: resumes from its cursor.
    docker.set_running(&web, false);
    docker.drop_logs(&web);
    docker.event("die", &web);
    tokio::time::sleep(Duration::from_millis(200)).await;
    docker.set_running(&web, true);
    docker.log(&web, 1, mock::ts(6), "restarted");
    docker.event("start", &web);
    assert_eq!(recv(&mut shop).await.0, "web/stdout restarted");
    assert_silent(&mut shop).await;

    // Shutdown closes the streams.
    tokio::time::timeout(Duration::from_secs(5), plugin.close())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(shop.stream.recv().await, Ok(None)));
}

#[tokio::test]
async fn docker_event_stream_reconnects() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("docker.sock");
    let docker = mock::MockDocker::serve(&socket);

    let mut plugin = Plugin::default();
    let (mut streams, _) = plugin
        .load_config(
            vec![stream("s", "docker", json!({"match": {"names": ["late"]}}))],
            vec![],
            vec![
                json!({
                    "docker": {"host": format!("unix://{}", socket.display())},
                    "reconnect_delay": "50ms",
                })
                .into(),
            ],
        )
        .await
        .unwrap();
    plugin.start().await.unwrap();
    let mut s = streams.pop().unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The daemon drops the event stream; a container starts meanwhile
    // and is picked up by the snapshot taken after reconnecting.
    let late = docker.add("late", &[], true);
    docker.log(&late, 1, mock::ts(1), "found by snapshot");
    docker.drop_events();
    assert_eq!(recv(&mut s).await.0, "found by snapshot");

    plugin.close().await.unwrap();
}

/// Minimal in-process Docker Engine API mock.
mod mock {
    use std::{
        collections::BTreeMap,
        convert::Infallible,
        path::Path,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use bytes::Bytes;
    use futures::{StreamExt, stream};
    use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
    use hyper::{Request, Response, StatusCode, body::Frame};
    use serde_json::json;
    use tokio::sync::broadcast;

    type Body = UnsyncBoxBody<Bytes, Infallible>;

    /// Timestamp `offset` seconds after the test started (stable within a test).
    pub fn ts(offset: i64) -> String {
        let t = base() + offset;
        chrono::DateTime::from_timestamp(t, 500_000_000)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
    }

    pub fn ts_at(time: SystemTime) -> String {
        chrono::DateTime::<chrono::Utc>::from(time)
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
    }

    pub fn ts_duration(offset: i64) -> Duration {
        Duration::new((base() + offset) as u64, 500_000_000)
    }

    fn base() -> i64 {
        static BASE: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
        *BASE.get_or_init(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                + 10
        })
    }

    struct Container {
        name: String,
        labels: BTreeMap<String, String>,
        running: bool,
        /// (unix seconds, stream type, frame)
        logs: Vec<(i64, u8, Bytes)>,
    }

    #[derive(Clone, Debug)]
    enum Signal {
        Log(String, Bytes),
        DropLogs(String),
        Event(String),
        DropEvents,
    }

    #[derive(Clone)]
    pub struct MockDocker {
        containers: Arc<Mutex<BTreeMap<String, Container>>>,
        signals: broadcast::Sender<Signal>,
        ready: Arc<AtomicBool>,
    }

    impl MockDocker {
        pub fn serve(path: &Path) -> Self {
            let mock = Self {
                containers: Arc::default(),
                signals: broadcast::channel(1024).0,
                ready: Arc::new(AtomicBool::new(true)),
            };
            let listener = tokio::net::UnixListener::bind(path).unwrap();
            let server = mock.clone();
            tokio::spawn(async move {
                loop {
                    let (conn, _) = listener.accept().await.unwrap();
                    let server = server.clone();
                    tokio::spawn(async move {
                        let service = hyper::service::service_fn(move |req| {
                            let server = server.clone();
                            async move { Ok::<_, Infallible>(server.handle(req)) }
                        });
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(hyper_util::rt::TokioIo::new(conn), service)
                            .await;
                    });
                }
            });
            mock
        }

        pub fn set_ready(&self, ready: bool) {
            self.ready.store(ready, Ordering::SeqCst);
        }

        pub fn add(&self, name: &str, labels: &[(&str, &str)], running: bool) -> String {
            let mut containers = self.containers.lock().unwrap();
            let id = format!("{:0>64x}", containers.len() + 1);
            containers.insert(
                id.clone(),
                Container {
                    name: name.into(),
                    labels: labels
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                    running,
                    logs: vec![],
                },
            );
            id
        }

        /// Append a line to the container's history and to live followers.
        pub fn log(&self, id: &str, stream_type: u8, ts: String, line: &str) {
            let payload = format!("{ts} {line}\n");
            let mut frame = vec![stream_type, 0, 0, 0];
            frame.extend((payload.len() as u32).to_be_bytes());
            frame.extend(payload.as_bytes());
            let frame = Bytes::from(frame);
            let secs = chrono::DateTime::parse_from_rfc3339(&ts)
                .unwrap()
                .timestamp();
            // History and live signal are updated under the same lock as the
            // one `logs` holds while subscribing, so a follower never gets a
            // line twice.
            let mut containers = self.containers.lock().unwrap();
            containers
                .get_mut(id)
                .unwrap()
                .logs
                .push((secs, stream_type, frame.clone()));
            let _ = self.signals.send(Signal::Log(id.into(), frame));
        }

        pub fn rename(&self, id: &str, name: &str, labels: &[(&str, &str)]) {
            let mut containers = self.containers.lock().unwrap();
            let container = containers.get_mut(id).unwrap();
            container.name = name.into();
            container.labels = labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        }

        pub fn set_running(&self, id: &str, running: bool) {
            self.containers.lock().unwrap().get_mut(id).unwrap().running = running;
        }

        pub fn drop_logs(&self, id: &str) {
            let _ = self.signals.send(Signal::DropLogs(id.into()));
        }

        pub fn drop_events(&self) {
            let _ = self.signals.send(Signal::DropEvents);
        }

        pub fn event(&self, action: &str, id: &str) {
            let event = json!({
                "Type": "container",
                "Action": action,
                "Actor": {"ID": id, "Attributes": {}},
            });
            let _ = self.signals.send(Signal::Event(format!("{event}\n")));
        }

        fn handle(&self, req: Request<hyper::body::Incoming>) -> Response<Body> {
            if !self.ready.load(Ordering::SeqCst) {
                return text(StatusCode::SERVICE_UNAVAILABLE, "docker daemon not ready");
            }
            let path = req.uri().path();
            // Strip the API version prefix: /v1.47/containers/json
            let path = match path.strip_prefix("/v") {
                Some(rest) => rest.find('/').map(|i| &rest[i..]).unwrap_or(path),
                None => path,
            };
            let query: BTreeMap<String, String> = req
                .uri()
                .query()
                .unwrap_or("")
                .split('&')
                .filter_map(|kv| kv.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let segments: Vec<&str> = path.trim_matches('/').split('/').collect();

            match segments.as_slice() {
                ["_ping"] => text(StatusCode::OK, "OK"),
                ["version"] => json_response(json!({"ApiVersion": "1.44", "Version": "25.0.0"})),
                ["containers", "json"] => {
                    let containers = self.containers.lock().unwrap();
                    let list: Vec<_> = containers
                        .iter()
                        .filter(|(_, c)| c.running)
                        .map(|(id, c)| {
                            json!({
                                "Id": id,
                                "Names": [format!("/{}", c.name)],
                                "Image": "img:latest",
                                "Labels": c.labels,
                                "State": "running",
                            })
                        })
                        .collect();
                    json_response(json!(list))
                }
                ["containers", id, "json"] => {
                    let containers = self.containers.lock().unwrap();
                    match containers.get(*id) {
                        Some(c) => json_response(json!({
                            "Id": id,
                            "Name": format!("/{}", c.name),
                            "Config": {"Image": "img:latest", "Labels": c.labels, "Tty": false},
                            "State": {"Running": c.running},
                        })),
                        None => not_found(),
                    }
                }
                ["containers", id, "logs"] => self.logs(id, &query),
                ["events"] => self.events(),
                _ => not_found(),
            }
        }

        fn logs(&self, id: &str, query: &BTreeMap<String, String>) -> Response<Body> {
            let since: i64 = query.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
            let want = |t: u8| match t {
                1 => query.get("stdout").is_some_and(|v| v == "true" || v == "1"),
                _ => query.get("stderr").is_some_and(|v| v == "true" || v == "1"),
            };
            // Subscribe and read history atomically (see `log`).
            let containers = self.containers.lock().unwrap();
            let rx = self.signals.subscribe();
            let history: Vec<Bytes> = match containers.get(id) {
                Some(c) => c
                    .logs
                    .iter()
                    .filter(|(secs, t, _)| *secs >= since && want(*t))
                    .map(|(_, _, frame)| frame.clone())
                    .collect(),
                None => return not_found(),
            };
            drop(containers);
            let id = id.to_string();
            let want_stdout = want(1);
            let want_stderr = want(2);
            let live = stream::unfold(rx, move |mut rx| {
                let id = id.clone();
                async move {
                    loop {
                        match rx.recv().await {
                            Ok(Signal::Log(cid, frame)) if cid == id => {
                                let wanted = match frame[0] {
                                    1 => want_stdout,
                                    _ => want_stderr,
                                };
                                if wanted {
                                    return Some((frame, rx));
                                }
                            }
                            Ok(Signal::DropLogs(cid)) if cid == id => return None,
                            Ok(_) => {}
                            Err(_) => return None,
                        }
                    }
                }
            });
            let body = stream::iter(history)
                .chain(live)
                .map(|b| Ok::<_, Infallible>(Frame::data(b)));
            Response::builder()
                .header("Content-Type", "application/vnd.docker.multiplexed-stream")
                .body(BodyExt::boxed_unsync(StreamBody::new(body)))
                .unwrap()
        }

        fn events(&self) -> Response<Body> {
            let rx = self.signals.subscribe();
            let body = stream::unfold(rx, |mut rx| async move {
                loop {
                    match rx.recv().await {
                        Ok(Signal::Event(event)) => return Some((Bytes::from(event), rx)),
                        Ok(Signal::DropEvents) => return None,
                        Ok(_) => {}
                        Err(_) => return None,
                    }
                }
            })
            .map(|b| Ok::<_, Infallible>(Frame::data(b)));
            Response::builder()
                .header("Content-Type", "application/json")
                .body(BodyExt::boxed_unsync(StreamBody::new(body)))
                .unwrap()
        }
    }

    fn text(status: StatusCode, body: &'static str) -> Response<Body> {
        Response::builder()
            .status(status)
            .body(Full::new(Bytes::from_static(body.as_bytes())).boxed_unsync())
            .unwrap()
    }

    fn json_response(value: serde_json::Value) -> Response<Body> {
        Response::builder()
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(value.to_string())).boxed_unsync())
            .unwrap()
    }

    fn not_found() -> Response<Body> {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header("Content-Type", "application/json")
            .body(
                Full::new(Bytes::from_static(br#"{"message":"No such container"}"#)).boxed_unsync(),
            )
            .unwrap()
    }
}
