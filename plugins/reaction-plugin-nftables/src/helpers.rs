use std::fmt::Display;

#[derive(PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
pub enum Version {
    IPv4,
    IPv6,
}
impl Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Version::IPv4 => "IPv4",
            Version::IPv6 => "IPv6",
        })
    }
}
