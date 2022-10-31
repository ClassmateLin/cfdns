use anyhow::Result;
use cfdns::{config::Config, dns_server::DnsServer, ping_server::PingServer, resource::Ipv4NetVec};
use dotenv::dotenv;
use log::{error, info};
use moka::future::Cache;
use std::{sync::Arc, time::Duration};
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

    let cache: Cache<String, bool> = Cache::builder()
        // Up to 10,000 entries.
        .max_capacity(10_000)
        // Create the cache.
        .build();

    let lock = Arc::new(RwLock::<(u32, u32)>::new((0, 9999)));

    let ipv4_net_list = match Ipv4NetVec::from_file(config.resource.ipv4_filepath) {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to read ip file, error:{}", e);
            return Err(e);
        }
    };
    let ipv4_net_list = Arc::new(ipv4_net_list);

    let dns_server = DnsServer::build(
        config.server,
        config.upstream,
        lock.clone(),
        cache,
        ipv4_net_list.clone(),
    )?;

    tokio::spawn(async move {
        info!("Start the DNS service after 5s...");
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = dns_server.run().await;
    });

    let ping_server = PingServer::build(config.ping, lock, ipv4_net_list)?;
    ping_server.run().await
}
