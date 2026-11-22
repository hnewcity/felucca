use super::*;
use crate::{
    Acl, AclMode,
    access::{TargetAcl, TargetPolicy},
};

// The field numbers here follow v2fly/v2ray-core app/router/config.proto.
// Encoding is deliberately independent of the production decoder.
fn varint(mut value: u64) -> Vec<u8> {
    let mut result = Vec::new();
    while value >= 128 {
        result.push(value as u8 | 128);
        value >>= 7;
    }
    result.push(value as u8);
    result
}
fn number(tag: u8, value: u64) -> Vec<u8> {
    [vec![tag << 3], varint(value)].concat()
}
fn bytes(tag: u8, value: &[u8]) -> Vec<u8> {
    [
        vec![tag << 3 | 2],
        varint(value.len() as u64),
        value.to_vec(),
    ]
    .concat()
}
fn cidr(ip: &str, prefix: u8) -> Vec<u8> {
    let ip: IpAddr = ip.parse().unwrap();
    let ip = match ip {
        IpAddr::V4(ip) => ip.octets().to_vec(),
        IpAddr::V6(ip) => ip.octets().to_vec(),
    };
    bytes(2, &[number(2, prefix.into()), bytes(1, &ip)].concat())
}
fn domain(kind: u64, value: &str, attributes: &[&str]) -> Vec<u8> {
    let mut value = [bytes(2, value.as_bytes()), number(1, kind)].concat();
    for attribute in attributes {
        value.extend(bytes(3, &bytes(1, attribute.as_bytes())));
    }
    bytes(2, &value)
}
fn resolver(kind: &str, data: Vec<u8>) -> GeoResolver {
    GeoResolver {
        files: [(kind.to_owned(), Arc::new(data))].into(),
        ..Default::default()
    }
}

#[test]
fn geoip_networks_handle_wire_order_masking_boundaries_and_mapped_v4() {
    let record = [
        cidr("192.0.2.19", 24), // datasets can carry unmasked addresses
        cidr("192.0.3.0", 24),  // adjacent ranges merge
        cidr("2001:db8::8", 32),
        bytes(1, b"TEST"),                  // country code need not be first
        vec![0x29, 0, 0, 0, 0, 0, 0, 0, 0], // unknown fixed64
    ]
    .concat();
    let geo = resolver("geoip", bytes(1, &record));
    let set = geo.ip("test").unwrap();
    assert_eq!(set.v4.len(), 1);
    for addr in [
        "192.0.2.0",
        "192.0.3.255",
        "::ffff:192.0.2.1",
        "2001:db8:ffff::1",
    ] {
        assert!(set.contains(addr.parse().unwrap()), "{addr}");
    }
    for addr in ["192.0.1.255", "192.0.4.0", "2001:db9::", "::1"] {
        assert!(!set.contains(addr.parse().unwrap()), "{addr}");
    }
    let source = Acl::parse_with_geo(AclMode::Allow, "geoip:TEST", &geo).unwrap();
    assert!(source.permits("192.0.2.1".parse().unwrap()));
    assert!(!source.permits("127.0.0.1".parse().unwrap()));
    let inverse = GeoIpSet::decode(&[record, number(3, 1)].concat()).unwrap();
    assert!(!inverse.contains("192.0.3.4".parse().unwrap()));
    assert!(inverse.contains("8.8.8.8".parse().unwrap()));
    let all = GeoIpSet::decode(&[cidr("0.0.0.0", 0), cidr("::", 0)].concat()).unwrap();
    assert!(all.contains("ffff:ffff:ffff:ffff::".parse().unwrap()));
    assert!(all.contains("255.255.255.255".parse().unwrap()));
}

#[test]
fn builtin_private_and_destination_deny_override_geosite_allow() {
    let record = [bytes(1, b"LOCAL"), domain(3, "localhost", &[])].concat();
    let geo = resolver("geosite", bytes(1, &record));
    let target = TargetAcl::compile_with_geo(
        &TargetPolicy {
            allow: vec!["geosite:local".into()],
            deny: vec!["geoip:private".into()],
            ports: vec![443],
        },
        &geo,
    )
    .unwrap();
    assert!(!target.permits("localhost", "127.0.0.1:443".parse().unwrap()));
    assert!(!target.permits("localhost", "[::ffff:127.0.0.1]:443".parse().unwrap()));
    assert!(!target.permits("localhost", "[::1]:443".parse().unwrap()));
    assert!(target.permits("localhost", "192.0.2.1:443".parse().unwrap()));
    assert!(!target.permits("localhost", "192.0.2.1:80".parse().unwrap()));
    assert!(!target.permits("evil.localhost", "192.0.2.1:443".parse().unwrap()));
    assert!(
        Acl::parse(AclMode::Allow, "geoip:private")
            .unwrap()
            .permits("100.64.0.1".parse().unwrap())
    );
}

#[test]
fn geosite_types_and_attribute_intersections() {
    let record = [
        bytes(1, b"TEST"),
        domain(0, "key.word", &["ads"]),
        domain(1, "^api[0-9]+\\.test$", &["api", "fast"]),
        domain(2, "Example.COM.", &["fast"]),
        domain(3, "exact.test", &[]),
        domain(1, r"^\D+\.nondigits$", &[]),
    ]
    .concat();
    let geo = resolver("geosite", bytes(1, &record));
    let all = geo.site("test").unwrap();
    for name in [
        "x-key.word.test",
        "api12.test",
        "example.com",
        "x.Example.COM.",
        "exact.test",
        "letters.nondigits",
    ] {
        assert!(all.matches(name), "{name}");
    }
    for name in [
        "keyXword.test",
        "api.test",
        "evilexample.com",
        "x.exact.test",
        "12.nondigits",
    ] {
        assert!(!all.matches(name), "{name}");
    }
    let selected = geo.site("test@fast@!api").unwrap();
    assert!(selected.matches("a.example.com"));
    assert!(!selected.matches("api12.test"));
    assert!(!selected.matches("exact.test"));
    assert!(geo.site("test@missing").is_err());
    assert!(geo.site("test@").is_err());
    assert!(geo.site("unknown").is_err());
    assert!(geo.site("arbitrary.dat:test").is_err());
}

#[test]
fn malformed_datasets_never_turn_into_empty_permit_lists() {
    for data in [
        vec![0],
        vec![0x0a, 10, 1],
        vec![
            0x0a, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 2,
        ],
        vec![0x0b], // unsupported protobuf group
        bytes(1, &[bytes(1, b"T"), cidr("127.0.0.1", 33)].concat()),
        bytes(1, &[bytes(1, b"T"), cidr("::1", 129)].concat()),
        bytes(1, &bytes(1, b"T")),
        bytes(
            1,
            &[bytes(1, b"T"), bytes(2, &bytes(1, &[1, 2, 3]))].concat(),
        ),
    ] {
        assert!(resolver("geoip", data).ip("T").is_err());
    }
    for domain in [
        domain(1, "[", &[]),
        domain(7, "bad", &[]),
        domain(3, "", &[]),
    ] {
        let data = bytes(1, &[bytes(1, b"T"), domain].concat());
        assert!(resolver("geosite", data).site("T").is_err());
    }
}

#[test]
fn configured_aliases_load_files_and_reject_missing_files_and_unsafe_aliases() {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "felucca-geo-{}-{}.dat",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let record = [bytes(1, b"T"), cidr("192.0.2.0", 24)].concat();
    std::fs::write(&path, bytes(1, &record)).unwrap();
    let config = GeoFiles {
        extra: [("custom".into(), path.clone())].into(),
        ..Default::default()
    };
    let geo = GeoResolver::new(&config).unwrap();
    std::fs::remove_file(&path).unwrap(); // loaded snapshots survive replacement
    assert!(
        geo.ip("custom:T")
            .unwrap()
            .contains("192.0.2.3".parse().unwrap())
    );
    assert!(GeoResolver::new(&config).is_err());
    for alias in ["../secret", "geoip", "geosite", "", "With Spaces"] {
        assert!(
            GeoResolver::new(&GeoFiles {
                extra: [(alias.into(), path.clone())].into(),
                ..Default::default()
            })
            .is_err()
        );
    }
}

#[test]
fn user_ownership_and_geo_references_are_validated_at_startup() {
    let mut config: crate::ServerConfig = toml::from_str(
        r#"
acl_mode = "deny"
acl_rules = "geoip:private"
[user.team]
acl_mode = "allow"
acl_rules = "192.0.2.0/24"
[client.a]
vkey = "k"
user = "team"
"#,
    )
    .unwrap();
    config.validate().unwrap();
    config.clients.get_mut("a").unwrap().user = Some("absent".into());
    assert!(
        config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("unknown user")
    );
    config.clients.get_mut("a").unwrap().user = Some("team".into());
    config.users.get_mut("team").unwrap().acl_rules = "geoip:unconfigured".into();
    assert!(
        config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("not configured")
    );
}
