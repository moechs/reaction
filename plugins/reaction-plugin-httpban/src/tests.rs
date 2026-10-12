use std::{
    error::Error,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use http::StatusCode;
use reaction_plugin::{ActionConfig, ActionImpl, Exec, PluginInfo};
use reqwest::Response;
use serde_json::json;
use tokio::time::sleep;

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
                        "set": "myset",
                        "action": "add",
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
                        "set": "myset",
                        "action": "delete",
                    })
                    .into(),
                    patterns: vec!["ip".into()],
                },
            ],
            vec![
                json!({
                    "downstream_count": 1,
                    "socket": {
                        "mode": "600",
                        "path": socket_path,
                    },
                    "graceful_shutdown_seconds": 0,
                    "upstreams": {},
                    "locations": {
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

    let result = integration_test_inner(socket_path, actions).await;

    // End
    plugin.close().await.unwrap();

    // Only test if it was successful here, because we need to properly
    // shutdown the server before panicking
    result.unwrap();
}

async fn integration_test_inner(
    socket_path: PathBuf,
    actions: Vec<ActionImpl>,
) -> Result<(), Box<dyn Error>> {
    // We must not panic inside this function
    // So we can't use assert* macros

    // Add some IPs
    let add_action = actions[0].clone();
    let delete_action = actions[1].clone();

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

    delete_action
        .tx
        .send(Exec {
            match_: vec!["2.3.4.5".into()],
            time: now(),
        })
        .await?;

    sleep(Duration::from_millis(100)).await;

    // Test server's response
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

        let result = client
            .execute(request)
            .await
            .map_err(|err| format!("err: {err}, request: {request_debug}"))?;

        let check_status =
            async |result: Response, status: StatusCode, expected_body: Option<&str>| {
                if result.status() != status {
                    return Err(format!(
                        "status is {} but {} was expected,
                        request: {request_debug},
                        response body: {:?}",
                        result.status(),
                        status,
                        result.text().await
                    ));
                }
                if let Some(expected_body) = expected_body {
                    let actual_body = result
                        .text()
                        .await
                        .map_err(|err| format!("err: {err}, request: {request_debug}"))?;
                    if actual_body != expected_body {
                        return Err(format!(
                            "body is {actual_body} but nomatch was expected,
                            request: {request_debug}",
                        ));
                    }
                }
                Ok(())
            };

        match expected {
            Ok(false) => {
                check_status(result, StatusCode::OK, Some("nomatch")).await?;
            }
            Ok(true) => {
                check_status(result, StatusCode::FORBIDDEN, Some("match")).await?;
            }
            Err(_) => {
                let status = result.status().as_u16();
                if status < 500 {
                    return Err(format!(
                        "status is {status} but >=500 was expected,
                        request: {request_debug},
                        body: {:?}",
                        result.text().await
                    )
                    .into());
                }

                let _body = result.text().await.map_err(|err| {
                    format!(
                        "err: {err},
                        request: {request_debug}"
                    )
                })?;
            }
        }
    }
    Ok(())
}
