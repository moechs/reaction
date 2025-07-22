use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use tracing::warn;

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
                    if let PatternType::IPv4 = self.pattern_type {
                        if let Cidr::IPv6(_) = cidr_normalized {
                            return Err(format!(
                                "An IPv4-only pattern can't have an IPv6 ({}) as an ignore",
                                cidr
                            ));
                        }
                    }
                    if let PatternType::IPv6 = self.pattern_type {
                        if let Cidr::IPv4(_) = cidr_normalized {
                            return Err(format!(
                                "An IPv6-only pattern can't have an IPv4 ({}) as an ignore",
                                cidr
                            ));
                        }
                    }
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
        let match_ip = match IpAddr::from_str(match_) {
            Ok(ip) => ip,
            Err(_) => return false,
        };
        self.ignore_cidr_normalized
            .iter()
            .all(|cidr| !cidr.includes(&match_ip))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cidr {
    IPv4((Ipv4Addr, Ipv4Addr)),
    IPv6((Ipv6Addr, Ipv6Addr)),
}

impl FromStr for Cidr {
    type Err = String;

    fn from_str(cidr: &str) -> Result<Self, Self::Err> {
        let (ip, mask) = cidr.split_once('/').ok_or(format!(
            "malformed IP/MASK. '{cidr}' doesn't contain any '/'"
        ))?;
        let ip = IpAddr::from_str(ip)
            .map_err(|err| format!("malformed IP '{ip}' in '{cidr}': {err}"))?;
        let mask_count = u32::from_str(mask)
            .map_err(|err| format!("malformed mask '{mask}' in '{cidr}': {err}"))?;

        if mask_count < 2 {
            return Err(format!("Can't have a network mask of 0 or 1. You're either ignoring all Internet or half of it."));
        } else if mask_count
            < (match ip {
                IpAddr::V4(_) => 8,
                IpAddr::V6(_) => 16,
            })
        {
            warn!("With a mask of {mask_count}, you're ignoring a big part of Internet. Are you sure you want to do this?");
        }

        let (ip_type, ip_bits) = match ip {
            IpAddr::V4(_) => ("IPv4", 32),
            IpAddr::V6(_) => ("IPv6", 128),
        };

        if mask_count > ip_bits {
            return Err(format!(
                "{ip_type} mask must be between 0 and {} inclusive. {mask_count} is too big.",
                ip_bits
            ));
        }

        match ip {
            IpAddr::V4(ipv4_addr) => {
                // Create bitmask
                let mask = match mask_count {
                    0 => 0u32,
                    n => !0u32 << (32 - n),
                };
                let mask = Ipv4Addr::from_bits(mask);
                // Normalize IP from mask
                let ipv4_addr = ipv4_addr & mask;

                Ok(Cidr::IPv4((ipv4_addr, mask)))
            }
            IpAddr::V6(ipv6_addr) => {
                // Create bitmask
                let mask = match mask_count {
                    0 => 0u128,
                    n => !0u128 << (128 - n),
                };
                let mask = Ipv6Addr::from_bits(mask);
                // Normalize IP from mask
                let ipv6_addr = ipv6_addr & mask;

                Ok(Cidr::IPv6((ipv6_addr, mask)))
            }
        }
    }
}

impl Cidr {
    fn includes(&self, ip: &IpAddr) -> bool {
        match self {
            Cidr::IPv4((network_ipv4, mask)) => match ip {
                IpAddr::V6(_) => false,
                IpAddr::V4(ipv4_addr) => *network_ipv4 == ipv4_addr & mask,
            },
            Cidr::IPv6((network_ipv6, mask)) => match ip {
                IpAddr::V4(_) => false,
                IpAddr::V6(ipv6_addr) => *network_ipv6 == ipv6_addr & mask,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr, Ipv6Addr},
        str::FromStr,
    };

    use super::Cidr;

    #[test]
    fn cidrv4_from_str() {
        assert_eq!(
            Ok(Cidr::IPv4((Ipv4Addr::new(192, 168, 1, 4), u32::MAX.into()))),
            Cidr::from_str("192.168.1.4/32")
        );
        // Test IP normalization from mask
        assert_eq!(
            Ok(Cidr::IPv4((
                Ipv4Addr::new(192, 168, 1, 0),
                Ipv4Addr::new(255, 255, 255, 0),
            ))),
            Cidr::from_str("192.168.1.4/24")
        );
        // Another ok-test "pour la route"
        assert_eq!(
            Ok(Cidr::IPv4((
                Ipv4Addr::new(1, 1, 0, 0),
                Ipv4Addr::new(255, 255, 0, 0),
            ))),
            Cidr::from_str("1.1.248.25/16")
        );
        // Errors
        assert!(Cidr::from_str("256.1.1.1/8").is_err());
        assert!(Cidr::from_str("1.1.1.1/0").is_err());
        assert!(Cidr::from_str("1.1.1.1/1").is_err());
        assert!(Cidr::from_str("1.1.1.1.1").is_err());
        assert!(Cidr::from_str("1.1.1.1/16/16").is_err());
    }

    #[test]
    fn cidrv6_from_str() {
        assert_eq!(
            Ok(Cidr::IPv6((
                Ipv6Addr::new(0xfe80, 0, 0, 0, 0xdf68, 0x2ee, 0xe4f9, 0xe68),
                u128::MAX.into()
            ))),
            Cidr::from_str("fe80::df68:2ee:e4f9:e68/128")
        );
        // Test IP normalization from mask
        assert_eq!(
            Ok(Cidr::IPv6((
                Ipv6Addr::new(0x2001, 0xdb8, 0x85a3, 0x9de5, 0, 0, 0, 0),
                Ipv6Addr::new(u16::MAX, u16::MAX, u16::MAX, u16::MAX, 0, 0, 0, 0),
            ))),
            Cidr::from_str("2001:db8:85a3:9de5::8a2e:370:7334/64")
        );
        // Another ok-test "pour la route"
        assert_eq!(
            Ok(Cidr::IPv6((
                Ipv6Addr::new(0x2001, 0xdb8, 0x85a3, 0x9d00, 0, 0, 0, 0),
                Ipv6Addr::new(
                    u16::MAX,
                    u16::MAX,
                    u16::MAX,
                    u16::MAX - u8::MAX as u16,
                    0,
                    0,
                    0,
                    0
                ),
            ))),
            Cidr::from_str("2001:db8:85a3:9d00::8a2e:370:7334/56")
        );
        assert!(Cidr::from_str("2001:db8:85a3:0:0:8a2e:370:7334/56").is_ok());
        assert!(Cidr::from_str("2001:DB8:85A3:0:0:8A2E:370:7334/56").is_ok());
        // Errors
        assert!(Cidr::from_str("2001:db8:85a3:0:0:8a2e:370:g334/56").is_err());
        assert!(Cidr::from_str("2001:db8:85a3:0:0:8a2e:370:7334/0").is_err());
        assert!(Cidr::from_str("2001:db8:85a3:0:0:8a2e:370:7334/1").is_err());
        assert!(Cidr::from_str("2001:db8:85a3:0:0:8a2e:370:7334:11/56").is_err());
        assert!(Cidr::from_str("2001:db8:85a3:0:0:8a2e:370:7334/11/56").is_err());
    }

    #[test]
    fn cidrv4_includes() {
        let cidr = Cidr::from_str("192.168.1.0/24").unwrap();
        assert!(cidr.includes(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 0))));
        assert!(cidr.includes(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(cidr.includes(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 234))));
        assert!(!cidr.includes(&IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))));
        assert!(!cidr.includes(&IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0xdf68, 0x2ee, 0xe4f9, 0xe68
        ),)));
    }

    #[test]
    fn cidrv6_includes() {
        let cidr = Cidr::from_str("2001:db8:85a3:9d00:0:8a2e:370:7334/56").unwrap();
        assert!(cidr.includes(&IpAddr::V6(Ipv6Addr::new(
            0x2001, 0x0db8, 0x85a3, 0x9d00, 0, 0, 0, 0
        ))));
        assert!(cidr.includes(&IpAddr::V6(Ipv6Addr::new(
            0x2001, 0x0db8, 0x85a3, 0x9da4, 0x34fc, 0x0d8b, 0xffff, 0x1111
        ))));
        assert!(!cidr.includes(&IpAddr::V6(Ipv6Addr::new(
            0x2001, 0x0db8, 0x85a3, 0xad00, 0, 0, 0, 1
        ))));
        assert!(!cidr.includes(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 0))));
    }
}
