use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use tracing::warn;

use cidr::Cidr;
use utils::*;

mod cidr;
mod utils;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum PatternType {
    #[default]
    #[serde(rename = "regex")]
    Regex,
    #[serde(rename = "ip")]
    Ip,
    #[serde(rename = "ipv4")]
    Ipv4,
    #[serde(rename = "ipv6")]
    Ipv6,
}

impl PatternType {
    pub fn is_default(&self) -> bool {
        *self == PatternType::default()
    }

    pub fn regex(&self) -> Option<String> {
        // Those orders of preference are very important for <ip>
        // patterns that have greedy catch-all regexes becore or after them,
        // for example: "Failed password .*<ip>.*"

        let num4 = [
            // Order is important, first is preferred.

            // first 25x
            "(?:25[0-5]",
            // then 2xx
            "2[0-4][0-9]",
            // then 1xx
            "1[0-9][0-9]",
            // then 0xx
            "[0-9][0-9]",
            // then 0x
            "[0-9])",
        ]
        .join("|");

        let numsix = "[0-9a-fA-F]{1,4}";

        let ipv4 = format!(r#"{num4}(?:\.{num4}){{3}}"#);

        #[allow(clippy::useless_format)]
        let ipv6 = [
            // We're unrolling all possibilities, longer IPv6 first,
            // to make it super-greedy,
            // more than an eventual .* before or after <ip> ,
            // that would "eat" its first or last blocks.

            // Order is important, first is preferred.

            // We put IPv4-suffixed regexes first
            format!(r#"::(?:ffff(?::0{{1,4}})?:)?{ipv4}"#),
            format!(r#"(?:{numsix}:){{1,4}}:{ipv4}"#),
            // Then link-local addresses with interface name
            format!(r#"fe80:(?::[0-9a-fA-F]{{0,4}}){{0,4}}%[0-9a-zA-Z]+"#),
            // Full IPv6
            format!("(?:{numsix}:){{7}}{numsix}"),
            // 1 block cut
            format!("(?:{numsix}:){{7}}:"),
            format!("(?:{numsix}:){{6}}:{numsix}"),
            format!("(?:{numsix}:){{5}}(?::{numsix}){{2}}"),
            format!("(?:{numsix}:){{4}}(?::{numsix}){{3}}"),
            format!("(?:{numsix}:){{3}}(?::{numsix}){{4}}"),
            format!("(?:{numsix}:){{2}}(?::{numsix}){{5}}"),
            format!("{numsix}:(?:(?::{numsix}){{6}})"),
            format!(":(?:(?::{numsix}){{7}})"),
            // 2 blocks cut
            format!("(?:{numsix}:){{6}}:"),
            format!("(?:{numsix}:){{5}}:{numsix}"),
            format!("(?:{numsix}:){{4}}(?::{numsix}){{2}}"),
            format!("(?:{numsix}:){{3}}(?::{numsix}){{3}}"),
            format!("(?:{numsix}:){{2}}(?::{numsix}){{4}}"),
            format!("{numsix}:(?:(?::{numsix}){{5}})"),
            format!(":(?:(?::{numsix}){{6}})"),
            // 3 blocks cut
            format!("(?:{numsix}:){{5}}:"),
            format!("(?:{numsix}:){{4}}:{numsix}"),
            format!("(?:{numsix}:){{3}}(?::{numsix}){{2}}"),
            format!("(?:{numsix}:){{2}}(?::{numsix}){{3}}"),
            format!("{numsix}:(?:(?::{numsix}){{4}})"),
            format!(":(?:(?::{numsix}){{5}})"),
            // 4 blocks cut
            format!("(?:{numsix}:){{4}}:"),
            format!("(?:{numsix}:){{3}}:{numsix}"),
            format!("(?:{numsix}:){{2}}(?::{numsix}){{2}}"),
            format!("{numsix}:(?:(?::{numsix}){{3}})"),
            format!(":(?:(?::{numsix}){{4}})"),
            // 5 blocks cut
            format!("(?:{numsix}:){{3}}:"),
            format!("(?:{numsix}:){{2}}:{numsix}"),
            format!("{numsix}:(?:(?::{numsix}){{2}})"),
            format!(":(?:(?::{numsix}){{3}})"),
            // 6 blocks cut
            format!("(?:{numsix}:){{2}}:"),
            format!("{numsix}::{numsix}"),
            format!(":(?:(?::{numsix}){{2}})"),
            // 7 blocks cut
            format!("{numsix}::"),
            format!("::{numsix}"),
            // special cuts
            // 8 blocks cut
            format!("::"),
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
    pub fn pattern_type(&self) -> PatternType {
        self.pattern_type
    }

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

    use chrono::Local;
    use tokio::{fs::read_to_string, task::JoinSet};

    use crate::{
        concepts::{Action, Duplicate, Filter, Pattern},
        daemon::{tests::TestBed, React},
    };

    use super::{Cidr, PatternIp, PatternType};

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

    pub const VALID_IPV4: [&str; 8] = [
        "252.4.92.250",
        "212.4.92.210",
        "112.4.92.110",
        "83.4.92.35",
        "83.4.92.0",
        "3.254.92.4",
        "1.2.3.4",
        "255.255.255.255",
    ];

    pub const VALID_IPV6: [&str; 42] = [
        // all accepted characters
        "0123:4567:89:ab:cdef:AB:CD:EF",
        // ipv6-mapped ipv4
        "::ffff:1.2.3.4",
        "ffff::1.2.3.4",
        // 8 blocks
        "1111:2:3:4:5:6:7:8888",
        // 7 blocks
        "::2:3:4:5:6:7:8888",
        "1111::3:4:5:6:7:8888",
        "1111:2::4:5:6:7:8888",
        "1111:2:3::5:6:7:8888",
        "1111:2:3:4::6:7:8888",
        "1111:2:3:4:5::7:8888",
        "1111:2:3:4:5:6::8888",
        "1111:2:3:4:5:6:7::",
        // 6 blocks
        "::3:4:5:6:7:8888",
        "1111::4:5:6:7:8888",
        "1111:2::5:6:7:8888",
        "1111:2:3::6:7:8888",
        "1111:2:3:4::7:8888",
        "1111:2:3:4:5::8888",
        "1111:2:3:4:5:6::",
        // 5 blocks
        "::4:5:6:7:8888",
        "1111::5:6:7:8888",
        "1111:2::6:7:8888",
        "1111:2:3::7:8888",
        "1111:2:3:4::8888",
        "1111:2:3:4:5::",
        // 4 blocks
        "::5:6:7:8888",
        "1111::6:7:8888",
        "1111:2::7:8888",
        "1111:2:3::8888",
        "1111:2:3:4::",
        // 3 blocks
        "::6:7:8888",
        "1111::7:8888",
        "1111:2::8888",
        "1111:2:3::",
        // 2 blocks
        "::7:8888",
        "1111::8888",
        "1111:2::",
        // 1 block
        "::8",
        "::8888",
        "1::",
        "1111::",
        // 0 block
        "::",
    ];

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

            for ip in VALID_IPV4 {
                assert2!(accepts_ipv4 == regex.is_match(ip));
            }

            assert2!(!regex.is_match(".1.2.3.4"));
            assert2!(!regex.is_match(" 1.2.3.4"));
            assert2!(!regex.is_match("1.2.3.4 "));
            assert2!(!regex.is_match("1.2. 3.4"));
            assert2!(!regex.is_match("257.2.3.4"));
            assert2!(!regex.is_match("1.2.3.4.5"));
            assert2!(!regex.is_match("1.2..4"));
            assert2!(!regex.is_match("1.2..3.4"));

            for ip in VALID_IPV6 {
                assert2!(accepts_ipv6 == regex.is_match(ip));
            }

            assert2!(!regex.is_match("1:"));
            assert2!(!regex.is_match("1:::"));
            assert2!(!regex.is_match("1:::2"));
            assert2!(!regex.is_match("1:2:3:4:5:6:7:8:9"));
            assert2!(!regex.is_match("1:23456:3:4:5:6:7:8"));
            assert2!(!regex.is_match("1:2:3:4:5:6:7:8:"));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ip_pattern_matches() {
        let mut join_set = JoinSet::new();

        for ip in VALID_IPV4.iter().chain(&VALID_IPV6) {
            for line in [
                format!("borned {ip} test"),
                //
                format!("right-unborned {ip} text"),
                format!("right-unborned {ip}text"),
                format!("right-unborned {ip}:"),
                //
                format!("left-unborned text {ip}"),
                format!("left-unborned text{ip}"),
                format!("left-unborned :{ip}"),
                //
                format!("full-unborned text {ip} text"),
                format!("full-unborned text{ip} text"),
                format!("full-unborned text {ip}text"),
                format!("full-unborned text{ip}text"),
                format!("full-unborned :{ip}:"),
                format!("full-unborned : {ip}:"),
            ] {
                join_set.spawn(tokio::spawn(async move {
                    let bed = TestBed::default();
                    let filter = Filter::new_static(
                        vec![Action::new(
                            vec!["sh", "-c", &format!("echo <ip> >> {}", &bed.out_file)],
                            None,
                            false,
                            "test",
                            "test",
                            "a1",
                            &bed.ip_patterns,
                            0,
                        )],
                        vec![
                            "^borned <ip> test",
                            "^right-unborned <ip>.*",
                            "^left-unborned .*<ip>",
                            "^full-unborned .*<ip>.*",
                        ],
                        None,
                        None,
                        "test",
                        "test",
                        Duplicate::Ignore,
                        &bed.ip_patterns,
                    );
                    let bed = bed.part2(filter, Local::now(), None).await;
                    assert_eq!(
                        bed.manager.handle_line(&line, Local::now()),
                        React::Trigger,
                        "line: {line}"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    assert_eq!(
                        &read_to_string(&bed.out_file).await.unwrap().trim_end(),
                        ip,
                        "line: {line}"
                    );
                }));
            }
        }

        join_set.join_all().await;
    }
}
