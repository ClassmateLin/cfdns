use anyhow::Result;
use cfdns::{
    config::Config,
    dns_server::DnsServer,
    ping_server::PingServer,
    resource::{DomainVec, Ipv4NetVec},
};
use dotenv::dotenv;
use log::error;
use std::sync::Arc;
use tokio::sync::RwLock;

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init();
    dotenv().ok();
    let config = match Config::from_file(None).await {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to read configuration, error:{}", e);
            return Err(e);
        }
    };

    let lock = Arc::new(RwLock::<(u32, u32)>::new((0, 9999)));

    let ipv4_net_list = match Ipv4NetVec::from_file(config.resource.ipv4_filepath) {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to read ip file, error:{}", e);
            return Err(e);
        }
    };

    let domain_list = match DomainVec::from_file(config.resource.domain_filepath) {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to read domain file, error:{}", e);
            return Err(e);
        }
    };

    let dns_server = DnsServer::build(config.server, config.upstream, lock.clone(), domain_list)?;

    tokio::spawn(async move {
        let _ = dns_server.run().await;
    });

    let ping_server = PingServer::build(config.ping, lock, ipv4_net_list)?;
    ping_server.run().await
}
