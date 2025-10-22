use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine, prelude::BASE64_STANDARD};
use iroh::{SecretKey, defaults};
use reaction_plugin::{
    ActionImpl, Hello, Manifest, PersistData, PluginInfo, RemoteResult, StreamImpl, Value,
    main_loop,
};
use remoc::{chmux::SendError, rtc};

#[tokio::main]
async fn main() {
    let plugin = Plugin::default();
    main_loop(plugin).await;
}

#[derive(Default)]
struct Plugin {
    data: Option<PersistData>,
}

impl PluginInfo for Plugin {
    async fn manifest(&mut self, data: PersistData) -> Result<Manifest, rtc::CallError> {
        self.data = Some(data);
        Ok(Manifest {
            hello: Hello::hello(),
            streams: BTreeSet::from(["cluster".into()]),
            actions: BTreeSet::from(["cluster_send".into()]),
        })
    }

    async fn stream_impl(
        &mut self,
        stream_name: String,
        stream_type: String,
        config: Value,
    ) -> RemoteResult<StreamImpl> {
        todo!()
    }

    async fn action_impl(
        &mut self,
        stream_name: String,
        filter_name: String,
        action_name: String,
        action_type: String,
        config: Value,
        patterns: Vec<String>,
    ) -> RemoteResult<ActionImpl> {
        todo!()
    }

    async fn finish_setup(&mut self) -> RemoteResult<()> {
        let data = self.data.as_mut().unwrap();
        let secret_key = secret_key(data).await;
        todo!()
    }

    async fn close(self) -> RemoteResult<()> {
        todo!()
    }
}

async fn secret_key(data: &mut PersistData) -> SecretKey {
    if let Some(key) = get_secret_key(data) {
        key
    } else {
        let key = SecretKey::generate(&mut rand::rng());
        set_secret_key(data, &key).await;
        key
    }
}

fn get_secret_key(data: &PersistData) -> Option<SecretKey> {
    match &data.persisted_data {
        Value::Object(map) => map.get("secret_key").and_then(|value| {
            if let Value::String(str) = value {
                let vec = BASE64_STANDARD.decode(str).ok()?;
                if vec.len() != 32 {
                    return None;
                }
                let mut bytes = [0u8; 32];
                for i in 0..32 {
                    bytes[i] = vec[i];
                }
                Some(SecretKey::from_bytes(&bytes))
            } else {
                None
            }
        }),
        _ => None,
    }
}

async fn set_secret_key(data: &mut PersistData, key: &SecretKey) {
    let mut current = match &data.persisted_data {
        Value::Object(map) => map.clone(),
        _ => BTreeMap::default(),
    };
    let base64 = BASE64_STANDARD.encode(key.to_bytes());
    current.insert("secret_key".into(), Value::String(base64));
    data.persist_data
        .send(Value::Object(current))
        .await
        .unwrap();
}
