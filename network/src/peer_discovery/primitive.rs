use crate::{primitives::PeerDiscoveryPrimitive, *};
use async_trait::async_trait;
use tokio::task::JoinHandle;

pub(crate) struct PeerDiscoveryPrimitiveImpl;

#[async_trait]
impl PeerDiscoveryPrimitive for PeerDiscoveryPrimitiveImpl {
    async fn serve(
        _network_config: &NetworkConfig,
        _initially_known_peers: Vec<Peer>,
    ) -> Result<(SharedKnownPeers, JoinHandle<Result<(), Error>>), Error> {
        unimplemented!();
    }
}