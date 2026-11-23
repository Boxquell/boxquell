//! The sandbox certificate authority. Guests trust its certificate, which lets
//! the gateway terminate TLS for hosts whose requests it has to see or change.

use anyhow::{Context, Result};
use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::crypto::ring::sign::any_supported_type;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use time::OffsetDateTime;

const LEAF_REUSE: Duration = Duration::from_secs(24 * 3600);

pub struct CertAuthority {
    issuer: Issuer<'static, KeyPair>,
    cert_pem: String,
    key_pem: String,
    /// One key for all leaf certificates; only the names differ.
    leaf_key: KeyPair,
    leaves: Mutex<HashMap<String, (Instant, Arc<CertifiedKey>)>>,
}

fn ca_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "boxquell sandbox CA");
    name.push(DnType::OrganizationName, "boxquell");
    params.distinguished_name = name;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params
}

impl CertAuthority {
    pub fn generate() -> Result<Self> {
        let key = KeyPair::generate()?;
        let cert_pem = ca_params().self_signed(&key)?.pem();
        Self::from_pem(cert_pem, &key.serialize_pem())
    }

    /// Rebuild the authority from the files written by [`generate`](Self::generate).
    pub fn from_pem(cert_pem: String, key_pem: &str) -> Result<Self> {
        let key = KeyPair::from_pem(key_pem).context("invalid CA key")?;
        Ok(Self {
            // Issuing only needs the subject name and key, both fixed by `ca_params`.
            issuer: Issuer::new(ca_params(), key),
            cert_pem,
            key_pem: key_pem.to_string(),
            leaf_key: KeyPair::generate()?,
            leaves: Mutex::new(HashMap::new()),
        })
    }

    /// Load `ca.crt` / `ca.key` from `dir`, creating them on first use.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let (cert_path, key_path) = (dir.join("ca.crt"), dir.join("ca.key"));
        if cert_path.exists() && key_path.exists() {
            return Self::from_pem(std::fs::read_to_string(&cert_path)?, &std::fs::read_to_string(&key_path)?);
        }
        let ca = Self::generate()?;
        std::fs::create_dir_all(dir)?;
        std::fs::write(&cert_path, &ca.cert_pem)?;
        write_private(&key_path, &ca.key_pem)?;
        Ok(ca)
    }

    /// The certificate guests have to trust.
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// Certificate and key the gateway presents to the guest as `host`.
    pub fn leaf(&self, host: &str) -> Result<Arc<CertifiedKey>> {
        let mut leaves = self.leaves.lock().unwrap();
        if let Some((made, leaf)) = leaves.get(host) {
            if made.elapsed() < LEAF_REUSE {
                return Ok(leaf.clone());
            }
        }
        let mut params = CertificateParams::new(vec![host.to_string()])?;
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, host);
        params.distinguished_name = name;
        let now = OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::days(1);
        params.not_after = now + time::Duration::days(30);
        let cert = params.signed_by(&self.leaf_key, &self.issuer)?;
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.leaf_key.serialize_der()));
        let signer = any_supported_type(&key).context("unsupported leaf key")?;
        let leaf = Arc::new(CertifiedKey::new(vec![CertificateDer::from(cert.der().to_vec())], signer));
        leaves.insert(host.to_string(), (Instant::now(), leaf.clone()));
        Ok(leaf)
    }
}

fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        file.write_all(contents.as_bytes())
    }
    #[cfg(not(unix))]
    {
        // The data directory lives in the user profile, which other users cannot read.
        std::fs::write(path, contents)
    }
}
