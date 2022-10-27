use crate::config::{ServerConf, UpstreamConf};
use anyhow::{anyhow, Result};
use bytes::{Bytes, BytesMut};
use domain::base::iana::{Class, Rcode};
use domain::base::{Message, MessageBuilder};
use domain::rdata;
use log::{debug, error, info};
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
    ttl: u32,
    domain_list: Arc<Vec<String>>,
}

impl DnsServer {
    pub fn build(
        conf: ServerConf,
        upstream_conf: UpstreamConf,
        rw_lock: Arc<RwLock<(u32, u32)>>,
        domain_list: Vec<String>,
    ) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            bind_sock_addr: format!("{:?}:{:?}", conf.host, conf.port).parse().unwrap(),
            upstream_sock_addr: format!("{:?}:{:?}", upstream_conf.host, upstream_conf.port)
                .parse()
                .unwrap(),
            ttl: conf.ttl,
            rw_lock,
            domain_list: Arc::new(domain_list),
        }))
    }

    // DNS 请求转发到上游DNS服务
    async fn request_upstream(self: Arc<Self>, qbuf: &Bytes) -> Result<Bytes> {
        let udp_addr = "0.0.0.0:0".parse::<SocketAddr>().unwrap();
        let udp_socket = UdpSocket::bind(udp_addr).await?;

        let mut rbuf = BytesMut::with_capacity(1024);
        rbuf.resize(1024, 0);
        udp_socket.send_to(qbuf, self.upstream_sock_addr).await?;

        let (len, _addr) = udp_socket.recv_from(&mut rbuf).await?;
        rbuf.resize(len, 0);
        let udp_addr = "0.0.0.0:0".parse::<SocketAddr>().unwrap();
        let udp_socket = UdpSocket::bind(udp_addr).await?;

        let mut rbuf = BytesMut::with_capacity(1024);
        rbuf.resize(1024, 0);
        udp_socket.send_to(qbuf, self.upstream_sock_addr).await?;

        let (len, _addr) = udp_socket.recv_from(&mut rbuf).await?;
        rbuf.resize(len, 0);
        Ok(rbuf.freeze())
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

        if qtype == domain::base::Rtype::A {
            // 域名匹配待优化
            let has = self
                .domain_list
                .iter()
                .filter(|e| e.contains(&question.qname().to_string()))
                .count()
                == 1;
            if has {
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
                return Ok(());
            }
        }

        debug!(
            "Send DNS request to upstream server, domain:{}...",
            question.qname().to_string()
        );
        // 不存在域名则请求上游服务器
        let rbuf = self.request_upstream(&qbuf).await?;
        let _ = client_socket.send_to(&rbuf, src).await;
        Ok(())
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
