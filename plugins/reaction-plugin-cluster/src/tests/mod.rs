use std::sync::{LazyLock, Mutex};

use serde_json::json;

mod conf;
mod e2e;

const SECRET_KEY_A: &str = "g7U1LPq2cgGSyk6CH_v1QpoXowSFKVQ8IcFljd_ZKGw=";
const PUBLIC_KEY_A: &str = "HhVh7ghqpXM9375HZ82OOeB504HBSS25wgug-1vUggY=";

const SECRET_KEY_B: &str = "5EgRjwIpqd60IXWCGg5dFTtxkI-0fS1PlhoIhUjh1eY=";
const PUBLIC_KEY_B: &str = "LPSQ9pS7m_5vvNC-fhoBNeL2-eS2Fd6aO4ImSnXp3lc=";

// Tests that spawn a database in current directory must be run one at a time
static TEST_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn stream_ok_port(port: u16) -> serde_json::Value {
    json!({
       "listen_port": port,
       "nodes": [{
        "public_key": PUBLIC_KEY_A,
       }],
       "message_timeout": "30m",
    })
}

fn stream_ok() -> serde_json::Value {
    stream_ok_port(2048)
}
