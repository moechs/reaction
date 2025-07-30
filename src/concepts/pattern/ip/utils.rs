use std::{
    net::{AddrParseError, IpAddr, Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

/// Normalize a string as an IP address.
/// IPv6-mapped IPv4 addresses are casted to IPv4.
pub fn normalize(ip: &str) -> Result<IpAddr, AddrParseError> {
    IpAddr::from_str(ip).map(normalize_ip)
}

/// Normalize a string as an IP address.
/// IPv6-mapped IPv4 addresses are casted to IPv4.
pub fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(ipv6) => match ipv6.to_ipv4_mapped() {
            Some(ipv4) => IpAddr::V4(ipv4),
            None => ip,
        },
    }
}

/// Creates an [`Ipv4Addr`] from a mask
pub fn mask_to_ipv4(mask_count: u8) -> Result<Ipv4Addr, String> {
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
pub fn mask_to_ipv6(mask_count: u8) -> Result<Ipv6Addr, String> {
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
