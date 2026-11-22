//! V2Ray GeoIP/GeoSite datasets, loaded only from operator-configured files.
//! nps uses these protobuf .dat files, not MaxMind MMDB.
use crate::ConfigError;
use felucca_i18n::{Message, Value as MessageValue};
use parking_lot::Mutex;
use regex_automata::meta::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Read,
    net::IpAddr,
    path::PathBuf,
    sync::Arc,
};

const MAX_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoFiles {
    #[serde(default)]
    pub geoip: Option<PathBuf>,
    #[serde(default)]
    pub geosite: Option<PathBuf>,
    /// Named additional datasets, referenced as geoip:alias:CODE or
    /// geosite:alias:CODE@attribute. A peer cannot choose arbitrary file paths.
    #[serde(default)]
    pub extra: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Default)]
pub struct GeoResolver {
    files: HashMap<String, Arc<Vec<u8>>>,
    ips: Mutex<HashMap<String, Arc<GeoIpSet>>>,
    sites: Mutex<HashMap<String, Arc<GeoSiteSet>>>,
}
impl GeoResolver {
    pub fn new(config: &GeoFiles) -> Result<Self, ConfigError> {
        if config.extra.len() > 16 {
            return Err(bad("at most 16 additional geodata files are supported"));
        }
        for name in config.extra.keys() {
            if name.is_empty()
                || name == "geoip"
                || name == "geosite"
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                return Err(bad("geodata aliases need lowercase letters, digits or '-'"));
            }
        }
        let mut files = HashMap::new();
        let mut loaded = HashMap::<PathBuf, Arc<Vec<u8>>>::new();
        let mut remaining = MAX_BYTES;
        let paths = config
            .geoip
            .iter()
            .map(|p| ("geoip", p))
            .chain(config.geosite.iter().map(|p| ("geosite", p)))
            .chain(config.extra.iter().map(|(name, p)| (name.as_str(), p)));
        for (name, path) in paths {
            let data = if let Some(data) = loaded.get(path) {
                data.clone()
            } else {
                let error = |e| {
                    ConfigError::invalid(Message::with(
                        "geodata {path}: {reason}",
                        [
                            MessageValue::new("path", path.display()),
                            MessageValue::new("reason", e),
                        ],
                    ))
                };
                let file = std::fs::File::open(path).map_err(error)?;
                let metadata = file.metadata().map_err(error)?;
                if !metadata.is_file() || metadata.len() > remaining as u64 {
                    return Err(bad(
                        "geodata must be regular files totaling at most 128 MiB",
                    ));
                }
                let mut data = Vec::new();
                file.take(remaining as u64 + 1)
                    .read_to_end(&mut data)
                    .map_err(error)?;
                if data.is_empty() || data.len() > remaining {
                    return Err(bad("empty or oversized geodata file"));
                }
                remaining -= data.len();
                let data = Arc::new(data);
                loaded.insert(path.clone(), data.clone());
                data
            };
            files.insert(name.to_owned(), data);
        }
        Ok(Self {
            files,
            ..Self::default()
        })
    }
    fn record(&self, kind: &str, selector: &str) -> Result<&[u8], ConfigError> {
        let (alias, code) = selector.split_once(':').unwrap_or((kind, selector));
        if code.is_empty()
            || code.len() > 128
            || !code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(bad("invalid geodata group"));
        }
        let data = self.files.get(alias).ok_or_else(|| {
            ConfigError::invalid(Message::with(
                "geodata file alias {alias:?} is not configured",
                [MessageValue::debug("alias", alias)],
            ))
        })?;
        for field in Fields::new(data) {
            let (tag, value) = field?;
            if tag != 1 {
                continue;
            }
            let record = value.bytes()?;
            for field in Fields::new(record) {
                let (tag, value) = field?;
                if tag == 1 && value.bytes()?.eq_ignore_ascii_case(code.as_bytes()) {
                    return Ok(record);
                }
            }
        }
        Err(ConfigError::invalid(Message::with(
            "geodata group {code:?} was not found",
            [MessageValue::debug("code", code)],
        )))
    }
    pub fn ip(&self, selector: &str) -> Result<Arc<GeoIpSet>, ConfigError> {
        if selector.len() > 256 {
            return Err(bad("GeoIP selector exceeds 256 bytes"));
        }
        let selector = selector.to_ascii_lowercase();
        let selector = selector.as_str();
        if let Some(set) = self.ips.lock().get(selector) {
            return Ok(set.clone());
        }
        let set = if selector.eq_ignore_ascii_case("private") {
            GeoIpSet::private()
        } else {
            GeoIpSet::decode(self.record("geoip", selector)?)?
        };
        let set = Arc::new(set);
        let mut cache = self.ips.lock();
        if cache.len() >= 1024 {
            return Err(bad("at most 1024 compiled GeoIP selectors are supported"));
        }
        cache.insert(selector.to_owned(), set.clone());
        Ok(set)
    }
    pub fn site(&self, selector: &str) -> Result<Arc<GeoSiteSet>, ConfigError> {
        if selector.len() > 512 {
            return Err(bad("GeoSite selector exceeds 512 bytes"));
        }
        let selector = selector.to_ascii_lowercase();
        let selector = selector.as_str();
        if let Some(set) = self.sites.lock().get(selector) {
            return Ok(set.clone());
        }
        let mut parts = selector.split('@');
        let code = parts.next().unwrap_or("");
        let attributes: Vec<_> = parts.map(str::to_ascii_lowercase).collect();
        if attributes.len() > 32
            || attributes.iter().any(|attribute| {
                attribute.strip_prefix('!').unwrap_or(attribute).is_empty()
                    || !attribute
                        .strip_prefix('!')
                        .unwrap_or(attribute)
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            })
        {
            return Err(bad("invalid geosite attribute selector"));
        }
        let set = Arc::new(GeoSiteSet::decode(
            self.record("geosite", code)?,
            &attributes,
        )?);
        let mut cache = self.sites.lock();
        if cache.len() >= 1024 {
            return Err(bad("at most 1024 compiled GeoSite selectors are supported"));
        }
        cache.insert(selector.to_owned(), set.clone());
        Ok(set)
    }
}

fn bad(message: &str) -> ConfigError {
    ConfigError::invalid(message)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeoIpSet {
    v4: Vec<(u128, u128)>,
    v6: Vec<(u128, u128)>,
    inverse: bool,
}
impl GeoIpSet {
    fn private() -> Self {
        let mut set = Self::default();
        for net in [
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "100.64.0.0/10",
            "169.254.0.0/16",
            "127.0.0.0/8",
            "::1/128",
            "fc00::/7",
            "fe80::/10",
        ] {
            let net: ipnet::IpNet = net.parse().expect("static private networks");
            set.push(net.addr(), net.prefix_len())
                .expect("static private prefix length");
        }
        set.merge();
        set
    }
    fn decode(record: &[u8]) -> Result<Self, ConfigError> {
        let mut set = Self::default();
        for field in Fields::new(record) {
            let (tag, value) = field?;
            match tag {
                2 => {
                    let mut ip = None;
                    let mut bits = 0;
                    for field in Fields::new(value.bytes()?) {
                        let (tag, value) = field?;
                        match tag {
                            1 => {
                                let data = value.bytes()?;
                                ip = Some(match data.len() {
                                    4 => IpAddr::from(
                                        <[u8; 4]>::try_from(data).expect("length checked"),
                                    ),
                                    16 => IpAddr::from(
                                        <[u8; 16]>::try_from(data).expect("length checked"),
                                    ),
                                    _ => return Err(bad("invalid GeoIP address size")),
                                });
                            }
                            2 => {
                                bits = u8::try_from(value.number()?)
                                    .map_err(|_| bad("invalid GeoIP prefix"))?
                            }
                            _ => {}
                        }
                    }
                    set.push(ip.ok_or_else(|| bad("missing GeoIP address"))?, bits)?;
                }
                3 => {
                    set.inverse = match value.number()? {
                        0 => false,
                        1 => true,
                        _ => return Err(bad("invalid GeoIP inverse flag")),
                    }
                }
                _ => {}
            }
        }
        if set.v4.is_empty() && set.v6.is_empty() {
            return Err(bad("GeoIP group has no networks"));
        }
        set.merge();
        Ok(set)
    }
    fn push(&mut self, ip: IpAddr, bits: u8) -> Result<(), ConfigError> {
        if self.v4.len() + self.v6.len() >= 1_048_576 {
            return Err(bad("GeoIP group exceeds 1048576 networks"));
        }
        let (ip, bits) = match ip {
            IpAddr::V6(ip) if bits >= 96 && ip.to_ipv4_mapped().is_some() => {
                (IpAddr::V4(ip.to_ipv4_mapped().expect("checked")), bits - 96)
            }
            _ => (ip, bits),
        };
        let (value, width, ranges) = match ip {
            IpAddr::V4(ip) => (u128::from(u32::from(ip)), 32, &mut self.v4),
            IpAddr::V6(ip) => (u128::from(ip), 128, &mut self.v6),
        };
        if bits > width {
            return Err(bad("GeoIP prefix exceeds its address width"));
        }
        let mask = if bits == width {
            0
        } else {
            u128::MAX >> (128 - (width - bits))
        };
        ranges.push((value & !mask, value | mask));
        Ok(())
    }
    fn merge(&mut self) {
        for ranges in [&mut self.v4, &mut self.v6] {
            ranges.sort_unstable();
            let mut merged: Vec<(u128, u128)> = Vec::with_capacity(ranges.len());
            for (start, end) in ranges.drain(..) {
                if let Some(last) = merged.last_mut()
                    && start <= last.1.saturating_add(1)
                {
                    last.1 = last.1.max(end);
                } else {
                    merged.push((start, end));
                }
            }
            *ranges = merged;
        }
    }
    pub fn contains(&self, address: IpAddr) -> bool {
        let (value, ranges) = match address.to_canonical() {
            IpAddr::V4(ip) => (u128::from(u32::from(ip)), &self.v4),
            IpAddr::V6(ip) => (u128::from(ip), &self.v6),
        };
        let contains = ranges
            .partition_point(|(start, _)| *start <= value)
            .checked_sub(1)
            .is_some_and(|index| value <= ranges[index].1);
        contains != self.inverse
    }
}

#[derive(Debug)]
pub struct GeoSiteSet {
    full: HashSet<String>,
    suffix: HashSet<String>,
    patterns: Option<Regex>,
}
impl GeoSiteSet {
    fn decode(record: &[u8], filters: &[String]) -> Result<Self, ConfigError> {
        let mut full = HashSet::new();
        let mut suffix = HashSet::new();
        let mut patterns = Vec::new();
        for field in Fields::new(record) {
            let (tag, value) = field?;
            if tag != 2 {
                continue;
            }
            let mut kind = 0;
            let mut name = None;
            let mut attributes = HashSet::new();
            for field in Fields::new(value.bytes()?) {
                let (tag, value) = field?;
                match tag {
                    1 => kind = value.number()?,
                    2 => {
                        name = Some(
                            std::str::from_utf8(value.bytes()?)
                                .map_err(|_| bad("invalid UTF-8 in GeoSite"))?
                                .trim()
                                .to_owned(),
                        )
                    }
                    3 => {
                        for field in Fields::new(value.bytes()?) {
                            let (tag, value) = field?;
                            if tag == 1 {
                                attributes.insert(
                                    std::str::from_utf8(value.bytes()?)
                                        .map_err(|_| bad("invalid GeoSite attribute"))?
                                        .trim()
                                        .to_ascii_lowercase(),
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !filters.iter().all(|filter| match filter.strip_prefix('!') {
                Some(name) => !attributes.contains(name),
                None => attributes.contains(filter),
            }) {
                continue;
            }
            let name = name
                .filter(|value| !value.is_empty())
                .ok_or_else(|| bad("empty GeoSite domain"))?;
            if name.len() > 4096 {
                return Err(bad("GeoSite pattern exceeds 4096 bytes"));
            }
            match kind {
                0 => {
                    let mut escaped = String::new();
                    for character in name.chars() {
                        if "\\.+*?[](){}|^$".contains(character) {
                            escaped.push('\\');
                        }
                        escaped.push(character);
                    }
                    patterns.push(escaped);
                }
                1 => patterns.push(name),
                2 => {
                    suffix.insert(name.trim_end_matches('.').to_ascii_lowercase());
                }
                3 => {
                    full.insert(name.trim_end_matches('.').to_ascii_lowercase());
                }
                _ => return Err(bad("unknown GeoSite domain type")),
            }
        }
        if full.is_empty() && suffix.is_empty() && patterns.is_empty() {
            return Err(bad("GeoSite selector matched no domains"));
        }
        let patterns = if patterns.is_empty() {
            None
        } else {
            Some(
                Regex::builder()
                    .syntax(regex_automata::util::syntax::Config::new().case_insensitive(true))
                    .build_many(&patterns)
                    .map_err(|_| bad("invalid or oversized GeoSite regex set"))?,
            )
        };
        Ok(Self {
            full,
            suffix,
            patterns,
        })
    }
    pub fn matches(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if self.full.contains(&host) || self.patterns.as_ref().is_some_and(|r| r.is_match(&host)) {
            return true;
        }
        let mut rest = host.as_str();
        loop {
            if self.suffix.contains(rest) {
                return true;
            }
            let Some((_, suffix)) = rest.split_once('.') else {
                return false;
            };
            rest = suffix;
        }
    }
}

#[derive(Clone, Copy)]
enum Value<'a> {
    Bytes(&'a [u8]),
    Number(u64),
    Other,
}
impl<'a> Value<'a> {
    fn bytes(self) -> Result<&'a [u8], ConfigError> {
        if let Self::Bytes(value) = self {
            Ok(value)
        } else {
            Err(bad("wrong geodata protobuf wire type"))
        }
    }
    fn number(self) -> Result<u64, ConfigError> {
        if let Self::Number(value) = self {
            Ok(value)
        } else {
            Err(bad("wrong geodata protobuf wire type"))
        }
    }
}
struct Fields<'a> {
    rest: &'a [u8],
}
impl<'a> Fields<'a> {
    fn new(rest: &'a [u8]) -> Self {
        Self { rest }
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8], ConfigError> {
        if count > self.rest.len() {
            return Err(bad("truncated geodata protobuf field"));
        }
        let (value, rest) = self.rest.split_at(count);
        self.rest = rest;
        Ok(value)
    }
    fn varint(&mut self) -> Result<u64, ConfigError> {
        let mut value = 0;
        for shift in (0..70).step_by(7) {
            let byte = self.take(1)?[0];
            if shift == 63 && byte > 1 {
                return Err(bad("geodata varint overflow"));
            }
            value |= u64::from(byte & 127) << shift;
            if byte & 128 == 0 {
                return Ok(value);
            }
        }
        Err(bad("geodata varint overflow"))
    }
    fn field(&mut self) -> Result<(u32, Value<'a>), ConfigError> {
        let tag = self.varint()?;
        let number = u32::try_from(tag >> 3).map_err(|_| bad("invalid geodata field number"))?;
        if number == 0 || number > 0x1fff_ffff {
            return Err(bad("invalid geodata field number"));
        }
        let value = match tag & 7 {
            0 => Value::Number(self.varint()?),
            1 => {
                self.take(8)?;
                Value::Other
            }
            2 => {
                let size =
                    usize::try_from(self.varint()?).map_err(|_| bad("geodata length overflow"))?;
                Value::Bytes(self.take(size)?)
            }
            5 => {
                self.take(4)?;
                Value::Other
            }
            _ => return Err(bad("unsupported geodata protobuf wire type")),
        };
        Ok((number, value))
    }
}
impl<'a> Iterator for Fields<'a> {
    type Item = Result<(u32, Value<'a>), ConfigError>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let result = self.field();
        if result.is_err() {
            self.rest = &[];
        }
        Some(result)
    }
}

#[cfg(test)]
#[path = "geodata_tests.rs"]
mod tests;
