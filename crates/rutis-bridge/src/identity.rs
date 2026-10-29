//! What a connection plugin compares when a far end presents a client
//! certificate: its fingerprint.

/// The SHA-256 fingerprint of a certificate's DER.
pub fn fingerprint(der: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(der).into()
}
