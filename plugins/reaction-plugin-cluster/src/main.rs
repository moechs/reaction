use std::collections::BTreeSet;

use reaction_plugin::{
    ActionImpl, Hello, Manifest, PersistData, PluginInfo, RemoteResult, StreamImpl, Value,
    main_loop,
};
use remoc::rtc;

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
        todo!()
    }

    async fn close(self) -> RemoteResult<()> {
        todo!()
    }
}
