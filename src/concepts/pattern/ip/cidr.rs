use std::{
    fmt::Display,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

use super::*;

/// Stores an IP and an associated mask.
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
        let ip = normalize(ip).map_err(|err| format!("malformed IP '{ip}' in '{cidr}': {err}"))?;
        let mask_count = u8::from_str(mask)
            .map_err(|err| format!("malformed mask '{mask}' in '{cidr}': {err}"))?;

        // Let's accept any mask size for now, as useless as it may seem
        // if mask_count < 2 {
        //     return Err("Can't have a network mask of 0 or 1. You're either ignoring all Internet or half of it.".into());
        // } else if mask_count
        //     < (match ip {
        //         IpAddr::V4(_) => 8,
        //         IpAddr::V6(_) => 16,
        //     })
        // {
        //     warn!("With a mask of {mask_count}, you're ignoring a big part of Internet. Are you sure you want to do this?");
        // }

        Self::from_ip_and_mask(ip, mask_count)
    }
}

impl Display for Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.network(), self.mask())
    }
}

impl Cidr {
    fn from_ip_and_mask(ip: IpAddr, mask_count: u8) -> Result<Self, String> {
        match ip {
            IpAddr::V4(mut ipv4_addr) => {
                // Create bitmask
                let mask = mask_to_ipv4(mask_count)?;
                // Normalize IP from mask
                ipv4_addr &= mask;

                Ok(Cidr::IPv4((ipv4_addr, mask)))
            }
            IpAddr::V6(mut ipv6_addr) => {
                let mask = mask_to_ipv6(mask_count)?;
                // Normalize IP from mask
                ipv6_addr &= mask;

                Ok(Cidr::IPv6((ipv6_addr, mask)))
            }
        }
    }

    /// Whether an IP is included in this IP CIDR.
    /// If IP is not the same version as CIDR, returns always false.
    pub fn includes(&self, ip: &IpAddr) -> bool {
        let ip = normalize_ip(*ip);
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

    fn network(&self) -> IpAddr {
        match self {
            Cidr::IPv4((network, _)) => IpAddr::from(*network),
            Cidr::IPv6((network, _)) => IpAddr::from(*network),
        }
    }

    fn mask(&self) -> u8 {
        let mut raw_mask = match self {
            Cidr::IPv4((_, mask)) => mask.to_bits() as u128,
            Cidr::IPv6((_, mask)) => mask.to_bits(),
        };
        let mut ret = 0;
        for _ in 0..128 {
            if raw_mask % 2 == 1 {
                ret += 1;
            }
            raw_mask >>= 1;
        }
        ret
    }
}

#[cfg(test)]
mod cidr_tests {
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
        // assert!(Cidr::from_str("1.1.1.1/0").is_err());
        // assert!(Cidr::from_str("1.1.1.1/1").is_err());
        // assert!(Cidr::from_str("1.1.1.1.1").is_err());
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
        // assert!(Cidr::from_str("2001:db8:85a3:0:0:8a2e:370:7334/0").is_err());
        // assert!(Cidr::from_str("2001:db8:85a3:0:0:8a2e:370:7334/1").is_err());
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

    #[test]
    fn cidr_display() {
        let cidrs = [
            ("192.168.1.4/32", "192.168.1.4/32"),
            ("192.168.1.4/24", "192.168.1.0/24"),
            ("1.1.248.25/16", "1.1.0.0/16"),
            ("fe80::df68:2ee:e4f9:e68/128", "fe80::df68:2ee:e4f9:e68/128"),
            (
                "2001:db8:85a3:9de5::8a2e:370:7334/64",
                "2001:db8:85a3:9de5::/64",
            ),
            (
                "2001:db8:85a3:9d00::8a2e:370:7334/56",
                "2001:db8:85a3:9d00::/56",
            ),
        ];
        for (from, to) in cidrs {
            assert_eq!(Cidr::from_str(from).unwrap().to_string(), to);
        }
    }
}
