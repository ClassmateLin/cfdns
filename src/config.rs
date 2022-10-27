use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::Ipv4Addr;

// DNS服务配置
#[derive(Debug, Serialize, Deserialize)]
pub struct ServerConf {
    pub host: Ipv4Addr,
    pub port: u16,
    pub ttl: u32,
}

// Ping服务配置
#[derive(Debug, Serialize, Deserialize)]
pub struct PingerConf {
    pub port: u16,
    pub workers: u16,
    pub times: u16,
    pub timeout: u64,
    pub interval: u64,
}

// 上游DNS服务配置
#[derive(Debug, Serialize, Deserialize)]
pub struct UpstreamConf {
    pub host: Ipv4Addr,
    pub port: u16,
}

// 资源文件配置
#[derive(Debug, Serialize, Deserialize)]
pub struct ResourceConf {
    pub ipv4_filepath: String,
    pub domain_filepath: String,
}

// 配置
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConf,
    pub upstream: UpstreamConf,
    pub ping: PingerConf,
    pub resource: ResourceConf,
}

impl Config {
    // 从配置文件中读取
    pub async fn from_file(path: Option<String>) -> Result<Self> {
        let path = match path {
            Some(p) => p,
            None => String::from("conf/config.toml"),
        };

        let conf_string = fs::read_to_string(path)?;

        Ok(toml::from_str::<Config>(&conf_string)?)
    }
}
