use std::{env, io::IsTerminal, time::Duration};

use tempfile::TempDir;
use tracing::Level;

use reaction::{cli::Format, client::request, daemon::daemon, protocol::Order};
use tokio::{
    fs::{read_to_string, write},
    time::sleep,
};

async fn config_with_cmd(config_path: &str, cmd: &str) {
    write(
        config_path,
        &("
{
    state_directory: '.',
    runtime_directory: '.',
    concurrency: 0,
    patterns: {
        num: { regex: '[0-9]+' },
    },
    streams: {
        stream1: {
            cmd: ['sh', '-c', '"
            .to_owned()
            + cmd
            + "'],
            eof: {
                behavior: 'exit',
            },
            filters: {
                filter1: {
                    regex: [
                        'here is <num>',
                        'regex that wont match <num>',
                    ],
                    retry: 2,
                    retryperiod: '2s',
                    duplicate: 'rerun',
                    actions: {
                        // Don't mix code and data at home!
                        // You may permit arbitrary execution from vilains,
                        // if your regex is permissive enough.
                        // This is OK only for testing purposes.
                        action1: {
                            cmd: ['sh', '-c', 'echo <num> >> ./out.txt'],
                        },
                        action2: {
                            cmd: ['sh', '-c', 'echo del <num> >> ./out.txt'],
                            after: '30s',
                            onexit: false,
                        },
                        action_oneshot: {
                            cmd: ['sh', '-c', 'echo oneshot <num> >> ./oneshot.txt'],
                            oneshot: true,
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
async fn simple() {
    let dir = TempDir::new().unwrap();
    env::set_current_dir(&dir).unwrap();

    let config_path = "config.jsonnet";
    let out_path = "./out.txt";
    let oneshot_path = "./oneshot.txt";
    let socket_path = "./reaction.sock";

    config_with_cmd(
        config_path,
        "for i in 12 24 36 24 36 12 12 24 56 67; do echo here is $i; sleep 0.01; done; sleep 0.15",
    )
    .await;

    write(out_path, "").await.unwrap();
    write(oneshot_path, "").await.unwrap();

    // Set the logger before running any code from the crate
    tracing_subscriber::fmt::fmt()
        .without_time()
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal())
        .with_max_level(Level::DEBUG)
        .try_init()
        .unwrap();

    // Run the daemon
    let handle = tokio::spawn(async move {
        daemon(Some(config_path.into()), None, Some(socket_path.into())).await
    });

    // Run the flushes

    // We sleep for the time the echoes are finished + a bit (100ms)
    let handle2 = tokio::spawn(async move {
        sleep(Duration::from_millis(200)).await;
        request(
            socket_path.into(),
            Format::JSON,
            None,
            vec![("num".into(), "24".into())],
            Order::Flush,
        )
        .await
    });

    let handle3 = tokio::spawn(async move {
        sleep(Duration::from_millis(220)).await;
        request(
            socket_path.into(),
            Format::JSON,
            None,
            vec![("num".into(), "56".into())],
            Order::Flush,
        )
        .await
    });

    let (daemon_exit, flush1, flush2) = tokio::join!(handle, handle2, handle3);
    assert!(daemon_exit.is_ok());
    assert!(daemon_exit.unwrap() == 1);
    assert!(flush1.is_ok());
    assert!(flush1.unwrap().is_ok());
    assert!(flush2.is_ok());
    assert!(flush2.unwrap().is_ok());

    assert_eq!(
        // 24 is encountered for the second time, then
        // 36 is encountered for the second time, then
        // 12 is encountered for the second time, then
        // 24 is flushed
        read_to_string(out_path).await.unwrap().trim(),
        "24\n36\n12\ndel 24".to_owned().trim()
    );

    // oneshot actions are also executed
    assert_eq!(
        read_to_string(oneshot_path).await.unwrap().trim(),
        "oneshot 24\noneshot 36\noneshot 12".to_owned().trim()
    );

    // Second part of the test
    // We test that persistence worked as intended
    // Both for matches and for flushes

    config_with_cmd(
        config_path,
        "sleep 0.02; for i in 12 24 36 56 67; do echo here is $i; sleep 0.01; done",
    )
    .await;

    write(out_path, "").await.unwrap();
    write(oneshot_path, "").await.unwrap();

    let daemon_exit = daemon(Some(config_path.into()), None, Some(socket_path.into())).await;
    assert!(daemon_exit == 1);

    // 36 trigger from DB
    // 12 trigger from DB
    // 12 match from DB + new match
    // 67 match from DB + new match
    let content = read_to_string(out_path).await.unwrap().trim().to_owned();
    let scenario1 = "36\n12\n12\n67".trim().to_owned();
    let scenario2 = "12\n36\n12\n67".trim().to_owned();
    assert!(
        content == scenario1 || content == scenario2,
        "content: {}\nscenario1: {}\nscenario2: {}",
        content,
        scenario1,
        scenario2
    );

    // triggers from the DB aren't executed for oneshot actions
    // only for new triggers
    // 12 match from DB + new match
    // 67 match from DB + new match
    assert_eq!(
        read_to_string(oneshot_path).await.unwrap().trim(),
        "oneshot 12\noneshot 67".to_owned().trim()
    );
}
