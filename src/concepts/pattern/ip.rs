use std::{
    fmt::Display,
    net::{AddrParseError, IpAddr, Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use tracing::warn;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum PatternType {
    #[default]
    Regex,
    Ip,
    Ipv4,
    Ipv6,
}

impl PatternType {
    pub fn is_default(&self) -> bool {
        *self == PatternType::default()
    }

    pub fn regex(&self) -> Option<String> {
        let num4 = r#"(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)"#;
        let num6 = r#"[0-9a-fA-F]{1,4}"#;
        let ipv4 = format!(r#"{num4}(?:\.{num4}){{3}}"#);
        let ipv6 = [
            format!(r#"(?:{num6}:){{7}}{num6}"#),
            format!(r#"(?:{num6}:){{1,7}}:"#),
            format!(r#"(?:{num6}:){{1,6}}:{num6}"#),
            format!(r#"(?:{num6}:){{1,5}}(?::{num6}){{1,2}}"#),
            format!(r#"(?:{num6}:){{1,4}}(?::{num6}){{1,3}}"#),
            format!(r#"(?:{num6}:){{1,3}}(?::{num6}){{1,4}}"#),
            format!(r#"(?:{num6}:){{1,2}}(?::{num6}){{1,5}}"#),
            format!(r#"{num6}:(?:(?::{num6}){{1,6}})"#),
            format!(r#":(?:(?::{num6}){{1,7}}|:)"#),
            format!(r#"fe80:(?::[0-9a-fA-F]{{0,4}}){{0,4}}%[0-9a-zA-Z]+"#),
            format!(r#"::(?:ffff(?::0{{1,4}})?:)?{ipv4}"#),
            format!(r#"(?:{num6}:){{1,4}}:{ipv4}"#),
        ]
        .join("|");
        match self {
            PatternType::Ipv4 => Some(ipv4),
            PatternType::Ipv6 => Some(ipv6),
            PatternType::Ip => Some(format!("{ipv4}|{ipv6}")),
            PatternType::Regex => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct PatternIp {
    #[serde(
        default,
        rename = "type",
        skip_serializing_if = "PatternType::is_default"
    )]
    pattern_type: PatternType,

    #[serde(default, rename = "ipv4mask")]
    ipv4_mask: Option<u8>,
    #[serde(default, rename = "ipv6mask")]
    ipv6_mask: Option<u8>,
    #[serde(skip)]
    ipv4_bitmask: Option<Ipv4Addr>,
    #[serde(skip)]
    ipv6_bitmask: Option<Ipv6Addr>,

    #[serde(default, rename = "ignorecidr", skip_serializing_if = "Vec::is_empty")]
    ignore_cidr: Vec<String>,
    #[serde(skip)]
    ignore_cidr_normalized: Vec<Cidr>,
}

impl PatternIp {
    /// Setup the IP-specific part of a Pattern.
    /// Returns an optional regex string if of type IP, else None
    /// Returns an error if one of:
    /// - the type is not IP but there is IP-specific config
    /// - the type is IP/IPv4/IPv6 and there is invalid IP-specific config
    /// - the type is IPv4 and there is IPv6-specific config
    /// - the type is IPv6 and there is IPv4-specific config
    pub fn setup(&mut self) -> Result<Option<String>, String> {
        match self.pattern_type {
            PatternType::Regex => {
                if self.ipv4_mask.is_some() {
                    return Err("ipv4mask is only allowed for patterns of `type: 'ip'`".into());
                }
                if self.ipv6_mask.is_some() {
                    return Err("ipv6mask is only allowed for patterns of `type: 'ip'`".into());
                }
                if !self.ignore_cidr.is_empty() {
                    return Err("ignorecidr is only allowed for patterns of `type: 'ip'`".into());
                }
            }

            PatternType::Ip | PatternType::Ipv4 | PatternType::Ipv6 => {
                if let Some(mask) = self.ipv4_mask {
                    self.ipv4_bitmask = Some(mask_to_ipv4(mask)?);
                }
                if let Some(mask) = self.ipv6_mask {
                    self.ipv6_bitmask = Some(mask_to_ipv6(mask)?);
                }

                for cidr in &self.ignore_cidr {
                    let cidr_normalized = Cidr::from_str(cidr)?;
                    let cidr_normalized_string = cidr_normalized.to_string();
                    if &cidr_normalized_string != cidr {
                        warn!("CIDR {cidr} should be rewritten in its normalized form: {cidr_normalized_string}");
                    }
                    self.ignore_cidr_normalized.push(cidr_normalized);
                }
                self.ignore_cidr = Vec::default();

                match self.pattern_type {
                    PatternType::Regex => (),
                    PatternType::Ip => (),
                    PatternType::Ipv4 => {
                        if self.ipv6_mask.is_some() {
                            return Err("An IPv4-only pattern can't have an ipv6mask".into());
                        }
                        for cidr in &self.ignore_cidr_normalized {
                            if let Cidr::IPv6(_) = cidr {
                                return Err(format!(
                                    "An IPv4-only pattern can't have an IPv6 ({}) as an ignore",
                                    cidr
                                ));
                            }
                        }
                    }

                    PatternType::Ipv6 => {
                        if self.ipv4_mask.is_some() {
                            return Err("An IPv6-only pattern can't have an ipv4mask".into());
                        }
                        for cidr in &self.ignore_cidr_normalized {
                            if let Cidr::IPv4(_) = cidr {
                                return Err(format!(
                                    "An IPv6-only pattern can't have an IPv4 ({}) as an ignore",
                                    cidr
                                ));
                            }
                        }
                    }
                }
            }
        }
        Ok(self.pattern_type.regex())
    }

    /// Whether the IP match is included in one of [`Self::ignore_cidr`]
    pub fn is_ignore(&self, match_: &str) -> bool {
        let match_ip = match IpAddr::from_str(match_) {
            Ok(ip) => ip,
            Err(_) => return false,
        };
        self.ignore_cidr_normalized
            .iter()
            .any(|cidr| cidr.includes(&match_ip))
    }

    /// Normalize the pattern.
    /// This should happen after checking on ignores.
    /// No-op when the pattern is not an IP.
    /// Otherwise BitAnd the IP with its configured mask,
    /// and add the /<mask>
    pub fn normalize(&self, match_: &mut String) {
        let ip = match self.pattern_type {
            PatternType::Regex => None,
            // Attempt to normalize only if type is IP*
            _ => normalize(match_)
                .ok()
                .and_then(|ip| match self.pattern_type {
                    PatternType::Ip => Some(ip),
                    PatternType::Ipv4 => match ip {
                        IpAddr::V4(_) => Some(ip),
                        _ => None,
                    },
                    PatternType::Ipv6 => match ip {
                        IpAddr::V6(_) => Some(ip),
                        _ => None,
                    },
                    _ => None,
                }),
        };
        if let Some(ip) = ip {
            *match_ = match ip {
                IpAddr::V4(addr) => match self.ipv4_bitmask {
                    Some(bitmask) => {
                        format!("{}/{}", addr & bitmask, self.ipv4_mask.unwrap_or(32))
                    }
                    None => addr.to_string(),
                },
                IpAddr::V6(addr) => match self.ipv6_bitmask {
                    Some(bitmask) => {
                        format!("{}/{}", addr & bitmask, self.ipv6_mask.unwrap_or(128))
                    }
                    None => addr.to_string(),
                },
            };
        }
    }
}

#[cfg(test)]
mod patternip_tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use crate::concepts::{pattern::ip::Cidr, Pattern};

    use super::{PatternIp, PatternType};

    #[test]
    fn test_setup_type_regex() {
        let mut regex_struct = PatternIp {
            pattern_type: PatternType::Regex,
            ..Default::default()
        };
        let copy = regex_struct.clone();
        // All default patterns is ok for regex type
        assert!(regex_struct.setup().is_ok());
        // Setup changes nothing
        assert_eq!(regex_struct, copy);

        // Any non-default field is err

        let mut regex_struct = PatternIp {
            pattern_type: PatternType::Regex,
            ipv4_mask: Some(24),
            ..Default::default()
        };
        assert!(regex_struct.setup().is_err());

        let mut regex_struct = PatternIp {
            pattern_type: PatternType::Regex,
            ipv6_mask: Some(64),
            ..Default::default()
        };
        assert!(regex_struct.setup().is_err());

        let mut regex_struct = PatternIp {
            pattern_type: PatternType::Regex,
            ignore_cidr: vec!["192.168.1/24".into()],
            ..Default::default()
        };
        assert!(regex_struct.setup().is_err());
    }

    #[test]
    fn test_setup_type_ip() {
        for pattern_type in [PatternType::Ip, PatternType::Ipv4, PatternType::Ipv6] {
            let mut ip_struct = PatternIp {
                pattern_type,
                ..Default::default()
            };
            assert!(ip_struct.setup().is_ok());

            let mut ip_struct = PatternIp {
                pattern_type,
                ipv4_mask: Some(24),
                ..Default::default()
            };
            match pattern_type {
                PatternType::Ipv6 => assert!(ip_struct.setup().is_err()),
                _ => {
                    assert!(ip_struct.setup().is_ok());
                    assert_eq!(
                        ip_struct.ipv4_bitmask,
                        Some(Ipv4Addr::new(255, 255, 255, 0))
                    );
                }
            }

            let mut ip_struct = PatternIp {
                pattern_type,
                ipv6_mask: Some(64),
                ..Default::default()
            };
            match pattern_type {
                PatternType::Ipv4 => assert!(ip_struct.setup().is_err()),
                _ => {
                    assert!(ip_struct.setup().is_ok());
                    assert_eq!(
                        ip_struct.ipv6_bitmask,
                        Some(Ipv6Addr::new(0xffff, 0xffff, 0xffff, 0xffff, 0, 0, 0, 0))
                    );
                }
            }

            let mut ip_struct = PatternIp {
                pattern_type,
                ignore_cidr: vec!["192.168.1.0/24".into()],
                ..Default::default()
            };
            match pattern_type {
                PatternType::Ipv6 => assert!(ip_struct.setup().is_err()),
                _ => {
                    assert!(ip_struct.setup().is_ok());
                    assert_eq!(
                        ip_struct.ignore_cidr_normalized,
                        vec![Cidr::IPv4((
                            Ipv4Addr::new(192, 168, 1, 0),
                            Ipv4Addr::new(255, 255, 255, 0)
                        ))]
                    );
                }
            }

            let mut ip_struct = PatternIp {
                pattern_type,
                ignore_cidr: vec!["::ffff:192.168.1.0/24".into()],
                ..Default::default()
            };
            match pattern_type {
                PatternType::Ipv6 => assert!(ip_struct.setup().is_err()),
                _ => {
                    assert!(ip_struct.setup().is_ok());
                    assert_eq!(
                        ip_struct.ignore_cidr_normalized,
                        vec![Cidr::IPv4((
                            Ipv4Addr::new(192, 168, 1, 0),
                            Ipv4Addr::new(255, 255, 255, 0)
                        ))]
                    );
                }
            }

            let mut ip_struct = PatternIp {
                pattern_type,
                ignore_cidr: vec!["2001:db8:85a3:9de5::8a2e:370:7334/64".into()],
                ..Default::default()
            };
            match pattern_type {
                PatternType::Ipv4 => assert!(ip_struct.setup().is_err()),
                _ => {
                    assert!(ip_struct.setup().is_ok());
                    assert_eq!(
                        ip_struct.ignore_cidr_normalized,
                        vec![Cidr::IPv6((
                            Ipv6Addr::new(0x2001, 0xdb8, 0x85a3, 0x9de5, 0, 0, 0, 0),
                            Ipv6Addr::new(u16::MAX, u16::MAX, u16::MAX, u16::MAX, 0, 0, 0, 0),
                        ))]
                    );
                }
            }
        }
    }

    #[test]
    fn test_is_ignore() {
        let mut ip_struct = PatternIp {
            pattern_type: PatternType::Ip,
            ignore_cidr: vec!["10.0.0.0/8".into(), "2001:db8:85a3:9de5::/64".into()],
            ..Default::default()
        };
        ip_struct.setup().unwrap();
        assert!(!ip_struct.is_ignore("prout"));
        assert!(!ip_struct.is_ignore("1.1.1.1"));
        assert!(!ip_struct.is_ignore("11.1.1.1"));
        assert!(!ip_struct.is_ignore("2001:db8:85a3:9de6::1"));
        assert!(ip_struct.is_ignore("10.1.1.1"));
        assert!(ip_struct.is_ignore("2001:db8:85a3:9de5::1"));
    }

    #[test]
    fn test_normalize() {
        let ipv4_32 = "1.1.1.1";
        let ipv4_32_norm = "1.1.1.1";
        let ipv4_24 = "1.1.1.0";
        let ipv4_24_norm = "1.1.1.0";
        let ipv4_24_mask = "1.1.1.0/24";
        let ipv6_128 = "2001:db8:85a3:9de5:0:0:01:02";
        let ipv6_128_norm = "2001:db8:85a3:9de5::1:2";
        let ipv6_64 = "2001:db8:85a3:9de5:0:0:0:0";
        let ipv6_64_norm = "2001:db8:85a3:9de5::";
        let ipv6_64_mask = "2001:db8:85a3:9de5::/64";

        for (ipv4_mask, ipv6_mask) in [(Some(24), None), (None, Some(64)), (Some(24), Some(64))] {
            let mut ip_struct = PatternIp {
                pattern_type: PatternType::Ip,
                ipv4_mask,
                ipv6_mask,
                ..Default::default()
            };
            ip_struct.setup().unwrap();

            let mut ipv4_32_modified = ipv4_32.to_string();
            let mut ipv4_24_modified = ipv4_24.to_string();
            let mut ipv6_128_modified = ipv6_128.to_string();
            let mut ipv6_64_modified = ipv6_64.to_string();

            ip_struct.normalize(&mut ipv4_32_modified);
            ip_struct.normalize(&mut ipv4_24_modified);
            ip_struct.normalize(&mut ipv6_128_modified);
            ip_struct.normalize(&mut ipv6_64_modified);

            match ipv4_mask {
                Some(_) => {
                    // modified with mask
                    assert_eq!(
                        ipv4_32_modified, ipv4_24_mask,
                        "ipv4mask: {:?}, ipv6mask: {:?}",
                        ipv4_mask, ipv6_mask
                    );
                    assert_eq!(
                        ipv4_24_modified, ipv4_24_mask,
                        "ipv4mask: {:?}, ipv6mask: {:?}",
                        ipv4_mask, ipv6_mask
                    );
                }
                None => {
                    // only normaized
                    assert_eq!(
                        ipv4_32_modified, ipv4_32_norm,
                        "ipv4mask: {:?}, ipv6mask: {:?}",
                        ipv4_mask, ipv6_mask
                    );
                    assert_eq!(
                        ipv4_24_modified, ipv4_24_norm,
                        "ipv4mask: {:?}, ipv6mask: {:?}",
                        ipv4_mask, ipv6_mask
                    );
                }
            }

            match ipv6_mask {
                Some(_) => {
                    // modified with mask
                    assert_eq!(
                        ipv6_128_modified, ipv6_64_mask,
                        "ipv4mask: {:?}, ipv6mask: {:?}",
                        ipv4_mask, ipv6_mask
                    );
                    assert_eq!(
                        ipv6_64_modified, ipv6_64_mask,
                        "ipv4mask: {:?}, ipv6mask: {:?}",
                        ipv4_mask, ipv6_mask
                    );
                }
                None => {
                    // only normalized
                    assert_eq!(
                        ipv6_128_modified, ipv6_128_norm,
                        "ipv4mask: {:?}, ipv6mask: {:?}",
                        ipv4_mask, ipv6_mask
                    );
                    assert_eq!(
                        ipv6_64_modified, ipv6_64_norm,
                        "ipv4mask: {:?}, ipv6mask: {:?}",
                        ipv4_mask, ipv6_mask
                    );
                }
            }
        }
    }

    #[test]
    fn test_ip_regexes() {
        for pattern_type in [PatternType::Ip, PatternType::Ipv4, PatternType::Ipv6] {
            let mut pattern = Pattern {
                ip: PatternIp {
                    pattern_type,
                    ..Default::default()
                },
                ..Default::default()
            };
            assert!(pattern.setup("zblorg").is_ok());
            let regex = pattern.compiled().unwrap();

            let accepts_ipv4 = pattern_type == PatternType::Ip || pattern_type == PatternType::Ipv4;
            let accepts_ipv6 = pattern_type == PatternType::Ip || pattern_type == PatternType::Ipv6;

            macro_rules! assert2 {
                ($a:expr) => {
                    assert!($a, "PatternType: {pattern_type:?}");
                };
            }

            assert2!(accepts_ipv4 == regex.is_match("1.2.3.4"));
            assert2!(accepts_ipv4 == regex.is_match("255.255.255.255"));

            assert2!(!regex.is_match(".1.2.3.4"));
            assert2!(!regex.is_match(" 1.2.3.4"));
            assert2!(!regex.is_match("1.2.3.4 "));
            assert2!(!regex.is_match("1.2. 3.4"));
            assert2!(!regex.is_match("257.2.3.4"));
            assert2!(!regex.is_match("1.2.3.4.5"));
            assert2!(!regex.is_match("1.2..4"));
            assert2!(!regex.is_match("1.2..3.4"));

            assert2!(accepts_ipv6 == regex.is_match("1:2:3:4:5:6:7:8"));
            assert2!(accepts_ipv6 == regex.is_match("::"));
            assert2!(accepts_ipv6 == regex.is_match("1::"));
            assert2!(accepts_ipv6 == regex.is_match("::1"));
            assert2!(accepts_ipv6 == regex.is_match("1:2::6:7:8"));
            assert2!(accepts_ipv6 == regex.is_match("0123:4567:89:ab:cdef:AB:CD:EF"));
            assert2!(accepts_ipv6 == regex.is_match("::ffff:1.2.3.4"));
            assert2!(accepts_ipv6 == regex.is_match("ffff::1.2.3.4"));

            assert2!(!regex.is_match("1:"));
            assert2!(!regex.is_match("1:::"));
            assert2!(!regex.is_match("1:::2"));
            assert2!(!regex.is_match("1:2:3:4:5:6:7:8:9"));
            assert2!(!regex.is_match("1:23456:3:4:5:6:7:8"));
            assert2!(!regex.is_match("1:2:3:4:5:6:7:8:"));
        }
    }
}

/// Normalize a string as an IP address.
/// IPv6-mapped IPv4 addresses are casted to IPv4.
fn normalize(ip: &str) -> Result<IpAddr, AddrParseError> {
    IpAddr::from_str(ip).map(normalize_ip)
}

/// Normalize a string as an IP address.
/// IPv6-mapped IPv4 addresses are casted to IPv4.
fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(ipv6) => match ipv6.to_ipv4_mapped() {
            Some(ipv4) => IpAddr::V4(ipv4),
            None => ip,
        },
    }
}

/// Creates an [`Ipv4Addr`] from a mask
fn mask_to_ipv4(mask_count: u8) -> Result<Ipv4Addr, String> {
    if mask_count > 32 {
        Err(format!(
            "an IPv4 mask must be 32 max. {mask_count} is too big."
        ))
    } else {
        let mask = match mask_count {
            0 => 0u32,
            n => u32::MAX << (32 - n),
        };
        let mask = Ipv4Addr::from_bits(mask);
        Ok(mask)
    }
}

/// Creates an [`Ipv4Addr`] from a mask
fn mask_to_ipv6(mask_count: u8) -> Result<Ipv6Addr, String> {
    if mask_count > 128 {
        Err(format!(
            "an IPv4 mask must be 128 max. {mask_count} is too big."
        ))
    } else {
        let mask = match mask_count {
            0 => 0u128,
            n => u128::MAX << (128 - n),
        };
        let mask = Ipv6Addr::from_bits(mask);
        Ok(mask)
    }
}

#[cfg(test)]
mod utils_tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::{mask_to_ipv4, mask_to_ipv6, normalize};

    #[test]
    fn test_normalize_ip() {
        assert_eq!(
            normalize("83.44.23.14"),
            Ok(IpAddr::V4(Ipv4Addr::new(83, 44, 23, 14)))
        );
        assert_eq!(
            normalize("2001:db8:85a3::8a2e:370:7334"),
            Ok(IpAddr::V6(Ipv6Addr::new(
                0x2001, 0xdb8, 0x85a3, 0x0, 0x0, 0x8a2e, 0x370, 0x7334
            )))
        );
        assert_eq!(
            normalize("::ffff:192.168.1.34"),
            Ok(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 34)))
        );
        assert_eq!(
            normalize("::ffff:1.2.3.4"),
            Ok(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)))
        );
    }

    #[test]
    fn test_mask_to_ipv4() {
        assert!(mask_to_ipv4(33).is_err());
        assert!(mask_to_ipv4(100).is_err());
        assert_eq!(mask_to_ipv4(16), Ok(Ipv4Addr::new(255, 255, 0, 0)));
        assert_eq!(mask_to_ipv4(24), Ok(Ipv4Addr::new(255, 255, 255, 0)));
        assert_eq!(mask_to_ipv4(25), Ok(Ipv4Addr::new(255, 255, 255, 128)));
        assert_eq!(mask_to_ipv4(26), Ok(Ipv4Addr::new(255, 255, 255, 192)));
        assert_eq!(mask_to_ipv4(32), Ok(Ipv4Addr::new(255, 255, 255, 255)));
    }

    #[test]
    fn test_mask_to_ipv6() {
        assert!(mask_to_ipv6(129).is_err());
        assert!(mask_to_ipv6(254).is_err());
        assert_eq!(
            mask_to_ipv6(56),
            Ok(Ipv6Addr::new(0xffff, 0xffff, 0xffff, 0xff00, 0, 0, 0, 0))
        );
        assert_eq!(
            mask_to_ipv6(64),
            Ok(Ipv6Addr::new(0xffff, 0xffff, 0xffff, 0xffff, 0, 0, 0, 0))
        );
        assert_eq!(
            mask_to_ipv6(112),
            Ok(Ipv6Addr::new(
                0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0
            ))
        );
        assert_eq!(
            mask_to_ipv6(128),
            Ok(Ipv6Addr::new(
                0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff
            ))
        );
    }
}

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
    fn includes(&self, ip: &IpAddr) -> bool {
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
