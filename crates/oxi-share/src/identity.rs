//! This device's TLS identity: a self-signed certificate whose SHA-256 fingerprint is the device id
//! other LocalSend devices remember. It is kept on disk so the id survives restarts.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

#[derive(Clone)]
pub struct Identity {
    pub cert: CertificateDer<'static>,
    key: Arc<PrivatePkcs8KeyDer<'static>>,
    /// Uppercase hex SHA-256 of the DER certificate, the format LocalSend uses.
    pub fingerprint: String,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity").field("fingerprint", &self.fingerprint).finish_non_exhaustive()
    }
}

pub fn fingerprint_of(cert_der: &[u8]) -> String {
    hex_upper(ring::digest::digest(&ring::digest::SHA256, cert_der).as_ref())
}

pub fn hex_upper(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02X}");
        s
    })
}

impl Identity {
    /// A new ECDSA P-256 key with a certificate valid from 1975 to 4096 (rcgen's default), so it
    /// never needs renewing. `CN=LocalSend User` like the official app: peers identify each other by
    /// fingerprint only.
    pub fn generate() -> io::Result<Self> {
        let key = rcgen::KeyPair::generate().map_err(io::Error::other)?;
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).map_err(io::Error::other)?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        params.distinguished_name.push(rcgen::DnType::CommonName, "LocalSend User");
        let cert = params.self_signed(&key).map_err(io::Error::other)?;
        Ok(Self::from_der(cert.der().to_vec(), key.serialize_der()))
    }

    fn from_der(cert: Vec<u8>, key: Vec<u8>) -> Self {
        let fingerprint = fingerprint_of(&cert);
        Self { cert: CertificateDer::from(cert), key: Arc::new(PrivatePkcs8KeyDer::from(key)), fingerprint }
    }

    /// Reads `cert.der` and `key.der` from `dir`, or makes and saves a new pair.
    pub fn load_or_create(dir: &Path) -> io::Result<Self> {
        let (cp, kp) = (dir.join("cert.der"), dir.join("key.der"));
        if let (Ok(cert), Ok(key)) = (fs::read(&cp), fs::read(&kp))
            && !cert.is_empty()
            && !key.is_empty()
        {
            let id = Self::from_der(cert, key);
            // A damaged pair would fail every handshake; replace it.
            if crate::tls::server_config(&id).is_ok() {
                return Ok(id);
            }
        }
        let id = Self::generate()?;
        fs::create_dir_all(dir)?;
        fs::write(&cp, id.cert.as_ref())?;
        fs::write(&kp, id.key.secret_pkcs8_der())?;
        Ok(id)
    }

    pub(crate) fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(self.key.clone_key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_uppercase_sha256_hex() {
        // SHA-256 of "abc".
        assert_eq!(fingerprint_of(b"abc"), "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD");
    }

    #[test]
    fn identity_persists() {
        let dir = std::env::temp_dir().join(format!("oxi-share-id-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let a = Identity::load_or_create(&dir).unwrap();
        let b = Identity::load_or_create(&dir).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_eq!(a.fingerprint.len(), 64);
        let _ = fs::remove_dir_all(&dir);
    }
}
