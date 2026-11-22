//! PF ABI from Apple's bsd/net/pfvar.h and FreeBSD's sys/net/pfvar.h.
//! DIOCNATLOOK uses _IOWR, not the write-only request code.
use std::{
    fs::File,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    os::fd::AsRawFd,
};

#[cfg(target_os = "macos")]
const PORT_WIDTH: usize = 4; // union pf_state_xport
#[cfg(target_os = "freebsd")]
const PORT_WIDTH: usize = 2;
const FAMILY: usize = 64 + 4 * PORT_WIDTH;
const SIZE: usize = FAMILY + 4; // 84 (Darwin), 76 (FreeBSD with trailing padding)
const NATLOOK: libc::c_ulong =
    0xc0000000 | ((SIZE as libc::c_ulong) << 16) | (b'D' as libc::c_ulong) << 8 | 23;

// Byte storage initializes every byte, including the C structure's padding.
// Alignment matches the pf_addr union; no pointer cast into a Rust enum/union.
#[repr(align(4))]
struct Natlook([u8; SIZE]);

impl Natlook {
    fn new(source: SocketAddr, destination: SocketAddr, direction: u8) -> io::Result<Self> {
        let mut data = Self([0; SIZE]);
        let source = SocketAddr::new(source.ip().to_canonical(), source.port());
        let destination = SocketAddr::new(destination.ip().to_canonical(), destination.port());
        match (source.ip(), destination.ip()) {
            (IpAddr::V4(source), IpAddr::V4(destination)) => {
                data.0[..4].copy_from_slice(&source.octets());
                data.0[16..20].copy_from_slice(&destination.octets());
                data.0[FAMILY] = libc::AF_INET as u8;
            }
            (IpAddr::V6(source), IpAddr::V6(destination)) => {
                data.0[..16].copy_from_slice(&source.octets());
                data.0[16..32].copy_from_slice(&destination.octets());
                data.0[FAMILY] = libc::AF_INET6 as u8;
            }
            _ => {
                return Err(super::invalid(
                    "PF source/destination address families differ",
                ));
            }
        }
        data.0[64..66].copy_from_slice(&source.port().to_be_bytes());
        data.0[64 + PORT_WIDTH..66 + PORT_WIDTH].copy_from_slice(&destination.port().to_be_bytes());
        data.0[FAMILY + 1] = libc::IPPROTO_TCP as u8;
        #[cfg(target_os = "macos")]
        {
            data.0[FAMILY + 3] = direction;
        }
        #[cfg(target_os = "freebsd")]
        {
            data.0[FAMILY + 2] = direction;
        }
        Ok(data)
    }
    fn destination(&self) -> io::Result<SocketAddr> {
        let port = u16::from_be_bytes(
            self.0[64 + 3 * PORT_WIDTH..66 + 3 * PORT_WIDTH]
                .try_into()
                .expect("fixed offset"),
        );
        let ip = match i32::from(self.0[FAMILY]) {
            libc::AF_INET => IpAddr::V4(Ipv4Addr::from(
                <[u8; 4]>::try_from(&self.0[48..52]).expect("fixed offset"),
            )),
            libc::AF_INET6 => IpAddr::V6(Ipv6Addr::from(
                <[u8; 16]>::try_from(&self.0[48..64]).expect("fixed offset"),
            )),
            _ => return Err(super::invalid("PF returned an unsupported address family")),
        };
        Ok(SocketAddr::new(ip, port))
    }
}

pub(super) fn lookup(
    file: &File,
    source: SocketAddr,
    destination: SocketAddr,
) -> io::Result<SocketAddr> {
    let mut last = io::Error::from(io::ErrorKind::NotFound);
    for direction in [2, 1] {
        // PF_OUT and PF_IN
        let mut query = Natlook::new(source, destination, direction)?;
        match ioctl(file, &mut query) {
            Ok(()) => return query.destination(),
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => last = error,
            Err(error) => return Err(error),
        }
    }
    Err(last)
}

// The only unsafe operation in this adapter. Keep the request code, buffer
// length and borrowed descriptor together instead of exposing a raw ioctl API.
#[allow(unsafe_code)]
fn ioctl(file: &File, query: &mut Natlook) -> io::Result<()> {
    // SAFETY: file owns a live descriptor for the duration of this call. The
    // request encodes SIZE, query is aligned writable storage of exactly SIZE,
    // and every byte has been initialized. The kernel does not retain it.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), NATLOOK, query.0.as_mut_ptr()) };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pf_query_layout_and_network_byte_order_match_kernel_abi() {
        assert_eq!(std::mem::size_of::<Natlook>(), SIZE);
        assert_eq!(std::mem::align_of::<Natlook>(), 4);
        #[cfg(target_os = "macos")]
        assert_eq!(NATLOOK, 0xc0544417);
        #[cfg(target_os = "freebsd")]
        assert_eq!(NATLOOK, 0xc04c4417);
        for (source, destination) in [
            ("192.0.2.1:51234", "198.51.100.9:8443"),
            ("[2001:db8::1]:51234", "[2001:db8::9]:8443"),
        ] {
            let source: SocketAddr = source.parse().unwrap();
            let destination: SocketAddr = destination.parse().unwrap();
            let mut query = Natlook::new(source, destination, 2).unwrap();
            assert_eq!(&query.0[64..66], &51234u16.to_be_bytes());
            let original = query.0[16..32].to_vec();
            query.0[48..64].copy_from_slice(&original);
            query.0[64 + 3 * PORT_WIDTH..66 + 3 * PORT_WIDTH]
                .copy_from_slice(&8443u16.to_be_bytes());
            assert_eq!(query.destination().unwrap(), destination);
            query.0[FAMILY] = 255;
            assert!(query.destination().is_err());
        }
    }
    #[test]
    fn real_ioctl_failure_is_returned_without_falling_back_to_listener() {
        let file = File::open("/dev/null").unwrap();
        let error = lookup(
            &file,
            "127.0.0.1:51234".parse().unwrap(),
            "127.0.0.1:15080".parse().unwrap(),
        )
        .unwrap_err();
        assert!(
            matches!(error.raw_os_error(), Some(libc::ENOTTY | libc::ENODEV)),
            "{error}"
        );
    }
}
