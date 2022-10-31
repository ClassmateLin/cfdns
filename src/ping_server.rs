use crate::config::PingerConf;
use anyhow::Result;
use async_channel::bounded;
use ipnet::Ipv4Net;
use log::{debug, error, info};
use std::net::{Ipv4Addr, SocketAddr};
use std::{sync::Arc, time::Duration};
use tokio::net::TcpStream;
use tokio::signal;
use tokio::sync::RwLock;
use tokio::time::{sleep, timeout, Instant};

// ping 服务
pub struct PingServer {
    pub ipv4_net: Arc<Vec<Ipv4Net>>,
    pub timeout: Duration,
    pub interval: Duration,
    pub port: u16,
    pub rw_lock: Arc<RwLock<(u32, u32)>>,
    pub workers: u16,
    pub times: u16,
}

impl PingServer {
    pub fn build(
        conf: PingerConf,
        rw_lock: Arc<RwLock<(u32, u32)>>,
        ipv4_net: Arc<Vec<Ipv4Net>>,
    ) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            timeout: Duration::from_millis(conf.timeout),
            interval: Duration::from_millis(conf.interval),
            port: conf.port,
            rw_lock,
            workers: conf.workers,
            times: conf.times,
            ipv4_net,
        }))
    }

    // tcping
    async fn ping(&self, ipv4_addr: Ipv4Addr) -> u128 {
        let socket_addr = format!("{}:{}", ipv4_addr, self.port)
            .parse::<SocketAddr>()
            .unwrap();
        match timeout(self.timeout, async move {
            let start = Instant::now();
            match TcpStream::connect(socket_addr).await {
                Ok(_) => start.elapsed().as_millis(),
                Err(_) => self.timeout.as_millis(),
            }
        })
        .await
        {
            Ok(t) => t,
            Err(_) => self.timeout.as_millis(),
        }
    }

    // 处理队列
    async fn processing_queue(
        self: Arc<Self>,
        worker_id: u16,
        ping_receiver: async_channel::Receiver<Ipv4Addr>,
        pong_sender: async_channel::Sender<(u32, u32)>,
        pinged_num: Arc<RwLock<u32>>,
    ) -> Result<()> {
        info!("Ping worker:{:?} is running...", worker_id);
        loop {
            tokio::select! {
                val = ping_receiver.recv() => {
                    match val {
                        Ok(ip) => {
                            {
                                let mut num = pinged_num.write().await;
                                *num += 1;
                            }
                            let mut ping_data = vec![];
                            let mut max_timeout = 0;
                            let mut min_timeout = 9999;
                            let mut sum = 0;
                            let timeout = self.ping(ip).await;
                            if timeout >= self.timeout.as_millis() {
                                continue;
                            }
                            ping_data.push(timeout);

                            for _ in 0..self.times - 1 {
                                let timeout = self.ping(ip).await;
                                ping_data.push(timeout);
                                sleep(self.interval).await;
                            }

                            for item in ping_data {
                                if item > max_timeout {
                                    max_timeout = item;
                                }
                                if item < min_timeout {
                                    min_timeout = item;
                                }
                                sum += item;
                            }
                           let avg =  match self.times > 3 {
                                true =>  {
                                    sum -= min_timeout;
                                    sum -= max_timeout;
                                    sum as u32 / (self.times - 2) as u32
                                },
                                false => {
                                    sum as u32 / self.times as u32
                                },
                            };

                            debug!("worker:{}, ping {:?}, min rtt:{:?}, max_rtt:{:?}, avg_rtt:{:?}", worker_id, ip, min_timeout, max_timeout, avg);

                            let _ = pong_sender.send((ip.into(), avg)).await;
                        },
                        Err(_) => {
                            pong_sender.close();
                            return Ok(());
                        },
                    }
                }
            }
        }
    }

    // 推送Ipv4Addr到队列中
    async fn push_queue(&self, ping_sender: async_channel::Sender<Ipv4Addr>) -> Result<()> {
        for ipv4net in self.ipv4_net.iter() {
            for host in ipv4net.hosts() {
                match ping_sender.send(host).await {
                    Ok(_) => {}
                    Err(e) => {
                        error!("Failed to send data:{:?}", e);
                        sleep(Duration::from_secs(10)).await;
                    }
                };
            }
        }
        Ok(())
    }

    // 主服务
    async fn serve(self: Arc<Self>) -> Result<()> {
        info!("Ping task in progress...");
        let pinged_num = Arc::new(RwLock::<u32>::new(0));
        let (ping_sender, ping_receiver) = bounded::<Ipv4Addr>(2048);
        let (pong_sender, pong_receiver) = bounded::<(u32, u32)>(2048);
        let that = self.clone();

        tokio::spawn(async move {
            let _ = that.push_queue(ping_sender).await;
        });
        let fast_ip_lock = Arc::new(RwLock::<(u32, u32)>::new((0, 9999)));
        for i in 1..self.workers + 1 {
            let that = self.clone();
            let ping = ping_receiver.clone();
            let pong = pong_sender.clone();
            let pnum = pinged_num.clone();
            tokio::spawn(async move {
                let _ = that.processing_queue(i, ping, pong, pnum).await;
            });
        }

        let mut interval = tokio::time::interval(Duration::from_secs(5));

        loop {
            tokio::select! {
               val =  pong_receiver.recv() => {
                    match val {
                        Ok((ip_u32, rtt)) => {
                            let flock = fast_ip_lock.clone();
                            {
                                let raw = flock.read().await;
                                if rtt >= raw.1 {
                                    continue;
                                }
                            }
                            let that = self.clone();
                            tokio::spawn(async move {
                                let mut write = that.rw_lock.write().await;
                                *write = (ip_u32, rtt);
                                info!("Change to a faster ip:{:?}, rtt: {}ms", Ipv4Addr::from(ip_u32), rtt);
                            });
                            let mut write = flock.write().await;
                            *write = (ip_u32, rtt);
                        },
                        Err(_) => {
                            if pong_sender.is_closed() {
                                return Ok(());
                            }

                        },
                    }
               }
               _  = interval.tick() => {
                        let number = pinged_num.read().await;
                        info!("Number of IPs that have been speed tested::{}", number);
              }


            }
        }
    }

    // 程序入口
    pub async fn run(self: Arc<Self>) -> Result<()> {
        loop {
            let that = self.clone();
            tokio::select! {
                _ = that.serve() => {
                    info!("All ip speed tests have been completed, wait for 7200 seconds to test the speed again.");
                    tokio::time::sleep(Duration::from_secs(7200)).await;
                }
                _ = signal::ctrl_c() => {
                    info!("Exit ping server...");
                    return Ok(());
                }
            }
        }
    }
}
