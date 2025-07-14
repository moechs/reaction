use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    ops::BitOr,
    str::FromStr,
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum PatternType {
    #[default]
    Regex,
    IP,
    IPv4,
    IPv6,
}

impl PatternType {
    pub fn is_default(&self) -> bool {
        *self == PatternType::default()
    }

    pub fn regex(&self) -> Option<String> {
        match self {
            PatternType::IPv4 => Some(r#"(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)(?:\.(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)){3}"#.into()),
            PatternType::IPv6 => Some(r#"(?:[0-9a-fA-F]{1,4}:){7,7}[0-9a-fA-F]{1,4}|(?:[0-9a-fA-F]{1,4}:){1,7}:|(?:[0-9a-fA-F]{1,4}:){1,6}:[0-9a-fA-F]{1,4}|(?:[0-9a-fA-F]{1,4}:){1,5}(?::[0-9a-fA-F]{1,4}){1,2}|(?:[0-9a-fA-F]{1,4}:){1,4}(?::[0-9a-fA-F]{1,4}){1,3}|(?:[0-9a-fA-F]{1,4}:){1,3}(?::[0-9a-fA-F]{1,4}){1,4}|(?:[0-9a-fA-F]{1,4}:){1,2}(?::[0-9a-fA-F]{1,4}){1,5}|[0-9a-fA-F]{1,4}:(?:(?::[0-9a-fA-F]{1,4}){1,6})|:(?:(?::[0-9a-fA-F]{1,4}){1,7}|:)|fe80:(?::[0-9a-fA-F]{0,4}){0,4}%[0-9a-zA-Z]{1,}|::(?:ffff(?::0{1,4}){0,1}:){0,1}(?:(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3}(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])|(?:[0-9a-fA-F]{1,4}:){1,4}:(?:(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3}(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])"#.into()),
            PatternType::IP =>  Some(r#"(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)(?:\.(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)){3}|(?:(?:[0-9a-fA-F]{1,4}:){7,7}[0-9a-fA-F]{1,4}|(?:[0-9a-fA-F]{1,4}:){1,7}:|(?:[0-9a-fA-F]{1,4}:){1,6}:[0-9a-fA-F]{1,4}|(?:[0-9a-fA-F]{1,4}:){1,5}(?::[0-9a-fA-F]{1,4}){1,2}|(?:[0-9a-fA-F]{1,4}:){1,4}(?::[0-9a-fA-F]{1,4}){1,3}|(?:[0-9a-fA-F]{1,4}:){1,3}(?::[0-9a-fA-F]{1,4}){1,4}|(?:[0-9a-fA-F]{1,4}:){1,2}(?::[0-9a-fA-F]{1,4}){1,5}|[0-9a-fA-F]{1,4}:(?:(?::[0-9a-fA-F]{1,4}){1,6})|:(?:(?::[0-9a-fA-F]{1,4}){1,7}|:)|fe80:(?::[0-9a-fA-F]{0,4}){0,4}%[0-9a-zA-Z]{1,}|::(?:ffff(?::0{1,4}){0,1}:){0,1}(?:(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3}(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])|(?:[0-9a-fA-F]{1,4}:){1,4}:(?:(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3}(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9]))"#.into()),
            PatternType::Regex => None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PatternIP {
    #[serde(
        default,
        rename = "type",
        skip_serializing_if = "PatternType::is_default"
    )]
    pattern_type: PatternType,
    #[serde(default, rename = "ignorecidr", skip_serializing_if = "Vec::is_empty")]
    ignore_cidr: Vec<String>,
    #[serde(skip)]
    ignore_cidr_normalized: Vec<Cidr>,
}

impl PatternIP {
    pub fn setup(&mut self) -> Result<Option<String>, String> {
        match self.pattern_type {
            PatternType::IP | PatternType::IPv4 | PatternType::IPv6 => {
                for cidr in &self.ignore_cidr {
                    let cidr_normalized = Cidr::from_str(cidr)?;
                    self.ignore_cidr_normalized.push(cidr_normalized);
                }
                self.ignore_cidr = Vec::default();
            }
            PatternType::Regex => {
                if !self.ignore_cidr.is_empty() {
                    return Err("ignorecidr is only allowed for patterns of `type: 'ip'`".into());
                }
            }
        }
        return Ok(self.pattern_type.regex());
    }

    pub fn is_ignore(&self, match_: &str) -> bool {
        // TODO
        todo!()
    }
}

#[derive(Clone, Debug)]
pub enum Cidr {
    IPv4((Ipv4Addr, Ipv4Addr)),
    IPv6((Ipv6Addr, Ipv6Addr)),
}

fn make_mask<T: BitOr>(mut mask_u32: u32) -> T {
    let mask = 0;
    while mask_u32 > 0 {
        mask |= 1 << mask_u32;
        mask_u32 -= 1;
    }
}

impl FromStr for Cidr {
    type Err = String;

    fn from_str(cidr: &str) -> Result<Self, Self::Err> {
        let (ip, mask) = cidr.split_once('/').ok_or(format!(
            "malformed IP/MASK. '{cidr}' doesn't contain any '/'"
        ))?;
        let ip = IpAddr::from_str(ip)
            .map_err(|err| format!("malformed IP '{ip}' in '{cidr}': {err}"))?;
        let mut mask_u32 = u32::from_str(mask)
            .map_err(|err| format!("malformed mask '{mask}' in '{cidr}': {err}"))?;

        let (ip_type, ip_bits) = match ip {
            IpAddr::V4(_) => ("IPv4", 32),
            IpAddr::V6(_) => ("IPv6", 128),
        };

        if mask_u32 > ip_bits {
            return Err(format!(
                "{ip_type} mask must be between 0 and {} inclusive. {mask_u32} is too big.",
                ip_bits
            ));
        }

        match ip {
            IpAddr::V4(ipv4_addr) => {
                let mask = match mask_u32 {
                    0 => 0u32,
                    n => !0u32 << (32 - n),
                };
                let mask = Ipv4Addr::from_bits(mask);
                Ok(Cidr::IPv4((ipv4_addr, mask)))
            }
            IpAddr::V6(ipv6_addr) => {
                let mask = match mask_u32 {
                    0 => 0u128,
                    n => !0u128 << (128 - n),
                };
                let mask = Ipv6Addr::from_bits(mask);
                Ok(Cidr::IPv6((ipv6_addr, mask)))
            }
        }

        // TODO normalize IP
    }
}

#[cfg(test)]
mod tests {}
