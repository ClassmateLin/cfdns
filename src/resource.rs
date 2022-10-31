use anyhow::Result;
use ipnet::Ipv4Net;
use std::fs;

pub struct Ipv4NetVec;

impl Ipv4NetVec {
    pub fn from_file(filepath: String) -> Result<Vec<Ipv4Net>> {
        let ipv4_net_list = fs::read_to_string(filepath)
            .unwrap()
            .lines()
            .filter_map(|e| e.parse::<Ipv4Net>().ok())
            .collect::<Vec<Ipv4Net>>();

        Ok(ipv4_net_list)
    }
}
