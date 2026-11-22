//! Password and bearer-token credentials. Raw credentials never enter logs.
use std::{io, num::NonZeroU32};

const ITERATIONS: u32 = 600_000;

pub fn hash_password(password: &[u8]) -> io::Result<String> {
    if !(12..=4096).contains(&password.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "passwords require 12..4096 bytes",
        ));
    }
    let nonce = felucca_proto::handshake::generate_nonce();
    let salt = &nonce[..16];
    let mut derived = [0; 32];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(ITERATIONS).expect("constant"),
        salt,
        password,
        &mut derived,
    );
    Ok(format!(
        "pbkdf2-sha256${ITERATIONS}${}${}",
        hex(salt),
        hex(&derived)
    ))
}

pub fn verify_password(encoded: &str, password: &[u8]) -> bool {
    let parts: Vec<_> = encoded.split('$').collect();
    if parts.len() != 4 || parts[0] != "pbkdf2-sha256" || password.len() > 4096 {
        return false;
    }
    let Some(iterations) = parts[1]
        .parse::<u32>()
        .ok()
        .filter(|n| *n == ITERATIONS)
        .and_then(NonZeroU32::new)
    else {
        return false;
    };
    let Some(salt) = unhex::<16>(parts[2]) else {
        return false;
    };
    let Some(expected) = unhex::<32>(parts[3]) else {
        return false;
    };
    ring::pbkdf2::verify(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        &salt,
        password,
        &expected,
    )
    .is_ok()
}

pub fn token() -> String {
    hex(&felucca_proto::handshake::generate_nonce())
}
pub fn digest(value: &[u8]) -> [u8; 32] {
    ring::digest::digest(&ring::digest::SHA256, value)
        .as_ref()
        .try_into()
        .expect("SHA256 size")
}
pub fn token_hash(token: &str) -> String {
    hex(&digest(token.as_bytes()))
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn unhex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut output = [0; N];
    for (out, pair) in output.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
        let digit = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        };
        *out = digit(pair[0])? * 16 + digit(pair[1])?;
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn password_hashes_use_unique_salts_and_reject_wrong_credentials_and_bad_costs() {
        let password = b"sample-password-with-entropy";
        let first = hash_password(password).unwrap();
        let second = hash_password(password).unwrap();
        assert_ne!(first, second);
        assert!(verify_password(&first, password));
        assert!(!verify_password(&first, b"wrong-password"));
        assert!(!verify_password(
            &first.replace("$600000$", "$999999999$"),
            password
        ));
        assert!(!verify_password("pbkdf2-sha256$600000$xx$xx", password));
        assert!(!verify_password(&first, &vec![0; 4097]));
        assert!(hash_password(b"tiny").is_err());
        let token = token();
        assert_eq!(token.len(), 64);
        assert_eq!(
            unhex::<32>(&token_hash(&token)),
            Some(digest(token.as_bytes()))
        );
    }
}
