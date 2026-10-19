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
    concurrency: 0,
    patterns: {
        ip: {
            type: 'ip',
            ipv6mask: 64,
            ignore: ['2.3.4.5'],
            ignorecidr:['fe80::/16'],
        },
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
                    regex: ['ip <ip>'],
                    retry: 2,
                    retryperiod: '2s',
                    duplicate: 'rerun',
                    actions: {
                        // Don't mix code and data at home!
                        // You may permit arbitrary execution from vilains,
                        // if your regex is permissive enough.
                        // This is OK only for testing purposes.
                        ipv4_1: {
                            cmd: ['sh', '-c', 'echo <ip> >> ./ipv4.txt'],
                            ipv4only: true,
                        },
                        ipv4_2: {
                            cmd: ['sh', '-c', 'echo del <ip> >> ./ipv4.txt'],
                            ipv4only: true,
                            after: '30s',
                            onexit: false,
                        },
                        ipv6_1: {
                            cmd: ['sh', '-c', 'echo <ip> >> ./ipv6.txt'],
                            ipv6only: true,
                        },
                        ipv6_2: {
                            cmd: ['sh', '-c', 'echo del <ip> >> ./ipv6.txt'],
                            ipv6only: true,
                            after: '30s',
                            onexit: false,
                        },
                        all_1: {
                            cmd: ['sh', '-c', 'echo <ip> >> ./out.txt'],
                        },
                        all_2: {
                            cmd: ['sh', '-c', 'echo del <ip> >> ./out.txt'],
                            after: '30s',
                            onexit: false,
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
async fn ip() {
    let dir = TempDir::new().unwrap();
    env::set_current_dir(&dir).unwrap();

    let config_path = "config.jsonnet";
    let out_path = "./out.txt";
    let ipv4_path = "./ipv4.txt";
    let ipv6_path = "./ipv6.txt";
    let socket_path = "./reaction.sock";

    config_with_cmd(
        config_path,
        "for i in 2.3.4.5 fe80::1; do echo ip $i; echo ip $i; done; for i in 1.2.3.4 204:31::1 5.5.5.5 1.2.3.4 204:31::1 5.5.5.5; do echo ip $i; sleep 0.01; done; sleep 0.15",
    ).await;

    write(out_path, "").await.unwrap();
    write(ipv4_path, "").await.unwrap();
    write(ipv6_path, "").await.unwrap();

    // Set the logger before running any code from the crate
    tracing_subscriber::fmt::fmt()
        .without_time()
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal())
        .with_max_level(Level::DEBUG)
        .try_init()
        .unwrap();

    // Run the daemon
    let handle =
        tokio::spawn(
            async move { daemon(Some(config_path.into()), None, socket_path.into()).await },
        );

    // Run the flushes

    // We sleep for the time the echoes are finished + a bit (100ms)
    let handle2 = tokio::spawn(async move {
        sleep(Duration::from_millis(160)).await;
        request(
            socket_path.into(),
            Format::JSON,
            None,
            vec![("ip".into(), "1.2.3.4".into())],
            Order::Flush,
        )
        .await
    });

    let handle3 = tokio::spawn(async move {
        sleep(Duration::from_millis(180)).await;
        request(
            socket_path.into(),
            Format::JSON,
            None,
            vec![("ip".into(), "204:31::/64".into())],
            Order::Flush,
        )
        .await
    });

    let (daemon_exit, flush1, flush2) = tokio::join!(handle, handle2, handle3);
    assert!(daemon_exit.is_ok());
    assert!(flush1.is_ok());
    assert!(flush2.is_ok());

    // tokio::time::sleep(Duration::from_secs(100)).await;

    assert_eq!(
        read_to_string(out_path).await.unwrap().trim(),
        [
            "1.2.3.4",
            "204:31::/64",
            "5.5.5.5",
            "del 1.2.3.4",
            "del 204:31::/64"
        ]
        .join("\n")
    );

    assert_eq!(
        read_to_string(ipv4_path).await.unwrap().trim(),
        ["1.2.3.4", "5.5.5.5", "del 1.2.3.4"].join("\n")
    );

    assert_eq!(
        read_to_string(ipv6_path).await.unwrap().trim(),
        ["204:31::/64", "del 204:31::/64"].join("\n")
    );
}
