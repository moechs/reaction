use std::{env, io::IsTerminal, time::Duration};

use tempfile::TempDir;
use tracing::Level;

use reaction::daemon::daemon;
use tokio::{
    fs::{read_to_string, remove_file, write},
    time::timeout,
};

async fn config_with_eof(config_path: &str, eof: &str) {
    write(
        config_path,
        &("
{
    state_directory: '.',
    runtime_directory: '.',
    patterns: {
        p: { regex: 'a' }
    },
    streams: {
        stream1: {
            cmd: ['sh', '-c', 'sleep 0.1; echo'],
            eof: "
            .to_owned()
            + eof
            + ",
            filters: {
                filter1: {
                    regex: ['^$'],
                    actions: {
                        action1: {
                            cmd: ['sh', '-c', 'echo line >> ./out.txt'],
                        },
                    }
                }
            }
        },
        stream2: {
            cmd: ['sleep', 'infinity'],
            eof: {
                behavior: 'ignore',
            },
            filters: {
                filter1: {
                    regex: ['^$'],
                    actions: {
                        action1: {
                            cmd: ['true'],
                        },
                    }
                }
            }
        }
    }
}"),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn eof() {
    // Set the logger before running any code from the crate
    tracing_subscriber::fmt::fmt()
        .without_time()
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal())
        .with_max_level(Level::DEBUG)
        .try_init()
        .unwrap();

    let dir = TempDir::new().unwrap();
    env::set_current_dir(&dir).unwrap();

    let config_path = "config.jsonnet";
    let out_path = "./out.txt";

    // First test: the stream starts 3 times, then make reaction exit

    let clean = || async move {
        let (_, _, res) = tokio::join!(
            remove_file("reaction.lock"),
            remove_file("reaction.sock"),
            write(out_path, "")
        );
        res.unwrap();
    };

    clean().await;

    config_with_eof(
        config_path,
        "{
            behavior: 'restart',
            after: '0ms',
            retry: 2,
            finally: 'exit',
        }",
    )
    .await;

    println!("{}", read_to_string(config_path).await.unwrap());

    // Run the daemon
    let result = timeout(
        Duration::from_millis(600),
        daemon(Some(config_path.into()), None, None),
    )
    .await;

    assert!(result.is_ok());

    assert_eq!(
        // command is started 3 times
        read_to_string(out_path).await.unwrap().trim(),
        "line\nline\nline\n".to_owned().trim()
    );

    // Second test: the stream starts 3 times, but doesn't make reaction exit

    clean().await;

    config_with_eof(
        config_path,
        "{
            behavior: 'restart',
            after: '0ms',
            retry: 2,
            finally: 'ignore',
        }",
    )
    .await;

    // Run the daemon
    let result = timeout(
        Duration::from_millis(600),
        daemon(Some(config_path.into()), None, None),
    )
    .await;

    assert!(result.is_err());

    assert_eq!(
        // command is started 3 times
        read_to_string(out_path).await.unwrap().trim(),
        "line\nline\nline".to_owned().trim()
    );

    // Third test: the stream starts 3 times, but with much delay.
    // It has only the time to start 2 times

    clean().await;

    config_with_eof(
        config_path,
        "{
            behavior: 'restart',
            after: '220ms',
            retry: 2,
            finally: 'exit',
        }",
    )
    .await;

    // Run the daemon
    let result = timeout(
        Duration::from_millis(600),
        daemon(Some(config_path.into()), None, None),
    )
    .await;

    assert!(result.is_err());

    assert_eq!(
        // command is started 2 times
        read_to_string(out_path).await.unwrap().trim(),
        "line\nline".to_owned().trim()
    );

    // Fourth test: reaction exits when the stream exits

    clean().await;

    config_with_eof(
        config_path,
        "{
            behavior: 'exit',
        }",
    )
    .await;

    // Run the daemon
    let result = timeout(
        Duration::from_millis(300),
        daemon(Some(config_path.into()), None, None),
    )
    .await;

    assert!(result.is_ok());

    assert_eq!(
        // command is started 1 time
        read_to_string(out_path).await.unwrap().trim(),
        "line".to_owned().trim()
    );
}
