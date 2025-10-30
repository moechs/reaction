use std::net::{SocketAddrV4, SocketAddrV6};

use iroh::Endpoint;
use reaction_plugin::RemoteResult;

use crate::Plugin;

const ALPN: [&[u8]; 1] = ["reaction_cluster_1".as_bytes()];

impl Plugin {
    pub async fn endpoint_init(&mut self) -> RemoteResult<()> {
        // while let Some((stream_name, stream)) = self.streams.pop_first() {
        for (stream_name, stream) in &self.streams {
            let mut builder = Endpoint::builder()
                .secret_key(stream.secret_key.clone())
                .alpns(ALPN.iter().map(|slice| slice.to_vec()).collect())
                .relay_mode(iroh::RelayMode::Disabled)
                .clear_discovery();

            if let Some(ip) = stream.bind_ipv4 {
                builder = builder.bind_addr_v4(SocketAddrV4::new(ip, stream.listen_port));
            }
            if let Some(ip) = stream.bind_ipv6 {
                builder = builder.bind_addr_v6(SocketAddrV6::new(ip, stream.listen_port, 0, 0));
            }

            let endpoint = builder.bind().await.map_err(|err| {
                format!("Could not create socket address for cluster {stream_name}: {err}")
            })?;
            self.endpoints.insert(stream_name.clone(), endpoint);
        }
        // We have no use of those parameters anymore
        self.streams = Default::default();
        Ok(())
    }
}
