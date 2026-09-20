use std::{
    error::Error,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use http::StatusCode;
use reaction_plugin::{ActionConfig, ActionImpl, Exec, PluginInfo};
use serde_json::json;

use crate::MyPlugin;

fn now() -> Duration {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
}

#[tokio::test]
async fn integration_test() {
    let tmp_dir = tempfile::TempDir::new().unwrap();
    let socket_path = tmp_dir.path().join("test.sock");
    // let socket_path = PathBuf::from("./test.sock");

    let mut plugin = MyPlugin::default();
    let (_streams, actions) = plugin
        .load_config(
            vec![],
            vec![
                ActionConfig {
                    stream_name: "s1".into(),
                    filter_name: "f1".into(),
                    action_name: "a1".into(),
                    action_type: "httpban".into(),
                    config: json!({
                        "add": "myset",
                    })
                    .into(),
                    patterns: vec!["ip".into()],
                },
                ActionConfig {
                    stream_name: "s1".into(),
                    filter_name: "f1".into(),
                    action_name: "a2".into(),
                    action_type: "httpban".into(),
                    config: json!({
                        "remove": "myset",
                    })
                    .into(),
                    patterns: vec!["ip".into()],
                },
            ],
            vec![
                json!({
                    "proxy_count": 1,
                    "socket": {
                        "mode": "600",
                        "path": socket_path,
                    },
                    "backends": {},
                    "domains": {
                        "exemple.fr": {
                            "case": {
                                "set": "myset",
                                "code": 403,
                                "inline": "match",
                            },
                            "default": {
                                "code": 200,
                                "inline": "nomatch",
                            },
                        },
                    },
                })
                .into(),
            ],
        )
        .await
        .unwrap();

    plugin.start().await.unwrap();

    eprintln!("before inner");
    let result = integration_test_inner(socket_path, actions).await;
    eprintln!("after  inner");

    // End
    eprintln!("before close");
    plugin.close().await.unwrap();
    eprintln!("after  close");

    eprintln!("before unwrap");
    result.unwrap();
    eprintln!("after  unwrap");
}

async fn integration_test_inner(
    socket_path: PathBuf,
    actions: Vec<ActionImpl>,
) -> Result<(), Box<dyn Error>> {
    // Add an IP
    let add_action = actions[0].clone();
    let remove_action = actions[1].clone();

    add_action
        .tx
        .send(Exec {
            match_: vec!["203.0.113.1".into()],
            time: now(),
        })
        .await?;

    add_action
        .tx
        .send(Exec {
            match_: vec!["198.51.100.0/24".into()],
            time: now(),
        })
        .await?;

    remove_action
        .tx
        .send(Exec {
            match_: vec!["2.3.4.5".into()],
            time: now(),
        })
        .await?;

    // Test server's response
    // let proxy = reqwest::Proxy::http(format!(
    //     "unix:{}",
    //     socket_path
    //         .to_str()
    //         .ok_or("socket path utf8 error or something")?
    // ))
    // .unwrap();
    // let client = reqwest::Client::builder().proxy(proxy).build()?;
    let client = reqwest::Client::builder()
        .unix_socket(socket_path)
        .timeout(Duration::from_secs(1))
        .build()?;

    let requests = vec![
        (
            client
                .get("http://exemple.fr")
                .header("X-Forwarded-For", "203.0.113.2")
                .build()?,
            Ok(false),
        ),
        (
            client
                .get("http://exemple.fr")
                .header("X-Forwarded-For", "203.0.113.1")
                .build()?,
            Ok(true),
        ),
        (
            client
                .get("http://exemple.fr")
                .header("X-Forwarded-For", "198.51.101.1")
                .build()?,
            Ok(false),
        ),
        (
            client
                .get("http://exemple.fr")
                .header("X-Forwarded-For", "198.51.100.1")
                .build()?,
            Ok(true),
        ),
        (
            client
                .get("http://exemple.fr")
                .header("X-Forwarded-For", "198.51.100.112")
                .build()?,
            Ok(true),
        ),
        (client.get("http://exemple.fr").build()?, Err(())),
    ];

    for (request, expected) in requests {
        let request_debug = format!("{request:?}");

        eprintln!("before request");
        let result = client
            .execute(request)
            .await
            .map_err(|err| format!("err: {err}, request: {request_debug}"))?;

        match expected {
            Ok(false) => {
                if result.status() != StatusCode::OK {
                    return Err(format!(
                        "status is {} but {} was expected: {request_debug}",
                        result.status(),
                        StatusCode::OK
                    )
                    .into());
                }

                let body = result
                    .text()
                    .await
                    .map_err(|err| format!("err: {err}, request: {request_debug}"))?;
                if body != "nomatch" {
                    return Err(format!(
                        "body is {body} but nomatch was expected: {request_debug}",
                    )
                    .into());
                }
            }
            Ok(true) => {
                if result.status() != StatusCode::FORBIDDEN {
                    return Err(format!(
                        "status is {} but {} was expected: {request_debug}",
                        result.status(),
                        StatusCode::FORBIDDEN
                    )
                    .into());
                }

                let body = result
                    .text()
                    .await
                    .map_err(|err| format!("err: {err}, request: {request_debug}"))?;
                if body != "match" {
                    return Err(
                        format!("body is {body} but match was expected: {request_debug}",).into(),
                    );
                }
            }
            Err(_) => {
                let status = result.status().as_u16();
                if status < 500 {
                    return Err(format!(
                        "status is {status} but >=500 was expected: {request_debug}"
                    )
                    .into());
                }

                let _body = result
                    .text()
                    .await
                    .map_err(|err| format!("err: {err}, request: {request_debug}"))?;
            }
        }
        eprintln!("after  request");
    }
    Ok(())
}
