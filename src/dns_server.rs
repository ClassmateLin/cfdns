use crate::config::{ServerConf, UpstreamConf};
use anyhow::{anyhow, Result};
use bytes::{Bytes, BytesMut};
use domain::base::iana::{Class, Rcode};
use domain::base::{Message, MessageBuilder};
use domain::rdata;
use ipnet::Ipv4Net;
use log::{debug, error, info};
use moka::future::Cache;
use std::net::Ipv4Addr;
use std::{net::SocketAddr, sync::Arc};
use tokio::net::UdpSocket;
use tokio::signal;
use tokio::sync::RwLock;

// DNS服务
pub struct DnsServer {
    bind_sock_addr: SocketAddr,
    upstream_sock_addr: SocketAddr,
    rw_lock: Arc<RwLock<(u32, u32)>>,
    cache: Cache<String, bool>,
    ttl: u32,
    ipv4_net: Arc<Vec<Ipv4Net>>,
}

impl DnsServer {
    pub fn build(
        conf: ServerConf,
        upstream_conf: UpstreamConf,
        rw_lock: Arc<RwLock<(u32, u32)>>,
        cache: Cache<String, bool>,
        ipv4_net: Arc<Vec<Ipv4Net>>,
    ) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            bind_sock_addr: format!("{:?}:{:?}", conf.host, conf.port).parse().unwrap(),
            upstream_sock_addr: format!("{:?}:{:?}", upstream_conf.host, upstream_conf.port)
                .parse()
                .unwrap(),
            ttl: conf.ttl,
            rw_lock,
            cache,
            ipv4_net,
        }))
    }

    // DNS 请求转发到上游DNS服务
    async fn request_upstream(&self, qbuf: &Bytes) -> Result<Bytes> {
        let udp_addr = "0.0.0.0:0".parse::<SocketAddr>().unwrap();
        let udp_socket = UdpSocket::bind(udp_addr).await?;

        let mut rbuf = BytesMut::with_capacity(1024);
        rbuf.resize(1024, 0);
        udp_socket.send_to(qbuf, self.upstream_sock_addr).await?;

        let (len, _addr) = udp_socket.recv_from(&mut rbuf).await?;
        
        rbuf.resize(len, 0);

        Ok(rbuf.freeze())
    }

    async fn return_fast_ip(
        &self,
        client_socket: Arc<UdpSocket>,
        src: SocketAddr,
        qmsg: Message<&Bytes>,
    ) -> Result<()> {
        let question = qmsg.sole_question()?;
        debug!(
            "Handling DNS requests, domain:{}...",
            question.qname().to_string()
        );
        let mut rmsg = MessageBuilder::from_target(BytesMut::with_capacity(1024))?
            .start_answer(&qmsg, Rcode::NoError)?;
        let header = rmsg.header_mut();
        header.set_ra(true);

        let data = {
            let raw = self.rw_lock.read().await;
            Ipv4Addr::from(raw.0)
        };
        rmsg.push((question.qname(), Class::In, self.ttl, rdata::A::new(data)))
            .unwrap();
        let _ = client_socket
            .send_to(rmsg.into_message().as_octets(), src)
            .await;
        Ok(())
    }

    // 处理DNS请求
    async fn process(
        self: Arc<Self>,
        client_socket: Arc<UdpSocket>,
        qbuf: Bytes,
        src: SocketAddr,
    ) -> Result<()> {
        let qmsg = Message::from_octets(&qbuf)?;
        let question = qmsg.sole_question()?;
        let qtype = question.qtype();

        if qtype != domain::base::Rtype::A {
            // 非A记录直接请求上游DNS服务器
            let rbuf = self.request_upstream(&qbuf).await?;
            let _ = client_socket.send_to(&rbuf, src).await;
            return Ok(());
        }

        // cdn 域名
        let is_cache = self
            .cache
            .get(question.qname().to_string().as_str())
            .is_some();
        if is_cache {
            self.return_fast_ip(client_socket, src, qmsg).await?;
            return Ok(());
        }

        let rbuf = self.request_upstream(&qbuf).await?;
        let rmsg = Message::from_octets(rbuf)?;
        let (_, ans, _, _) = rmsg.sections()?;
        let mut ip = Ipv4Addr::new(127, 0, 0, 1);

        for rr in ans.flatten() {
            if rr.rtype() != domain::base::Rtype::A {
                continue;
            }
            if let Ok(record) = rr.to_record::<domain::rdata::rfc1035::A>() {
                if record.is_some() {
                    let record = record.unwrap();
                    ip = record.data().to_string().as_str().parse::<Ipv4Addr>()?;
                    break;
                }
            }
        }
        let bool = self.contains(ip);
        if bool {
            self.cache.insert(question.qname().to_string(), true).await;
            self.return_fast_ip(client_socket, src, qmsg).await?;
            return Ok(());
        }

        let _ = client_socket.send_to(&rmsg.into_octets(), src).await;

        Ok(())
    }

    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        for item in self.ipv4_net.iter() {
            if item.contains(&ip) {
                return true;
            }
        }
        false
    }

    // 服务入口
    // 监听配置的DNS服务SocketAddr, 每收到一个请求, 在子task中处理。
    pub async fn run(self: Arc<Self>) -> Result<()> {
        let socket = match UdpSocket::bind(self.bind_sock_addr).await {
            Ok(sock) => {
                info!("Dns server listen on[{:?}]...", self.bind_sock_addr);
                Arc::new(sock)
            }
            Err(e) => {
                error!(
                    "Unable to bind address[{:?}], error:{}",
                    self.bind_sock_addr, e
                );
                return Err(anyhow!(e));
            }
        };

        loop {
            let socket = socket.clone();
            let mut buf = BytesMut::with_capacity(1024);
            buf.resize(1024, 0);

            tokio::select! {
                res = socket.recv_from(&mut buf) => {
                    let (len, src) = match res {
                        Ok(r) => r,
                        Err(e) => {
                            error!("Fail to read socket data, error:{}", e);
                            continue;
                        }
                    };
                    buf.resize(len, 0);
                    let that = self.clone();
                    tokio::spawn(async move {
                        let _ = that.process(socket, buf.freeze(), src).await;
                    });

                }
                _ = signal::ctrl_c() => {
                    info!("Exit dns server...");
                    return Ok(());
                }
            }
        }
    }
}
