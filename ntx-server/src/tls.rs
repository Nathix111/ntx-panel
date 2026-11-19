use crate::config::write_file_atomic;
use anyhow::{Context, Result};
use rcgen::{CertificateParams, DnType, KeyPair};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::path::Path;
use tonic::transport::{Identity, ServerTlsConfig};

#[derive(Clone, Debug)]
pub struct TlsMaterial {
    pub cert_pem: String,
    pub key_pem: String,
    pub fingerprint_sha256: String,
}

/// Format a 64-char hex SHA-256 fingerprint as `AA:BB:CC:...`.
pub fn format_fingerprint(hex_fp: &str) -> String {
    let clean: String = hex_fp
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_uppercase();

    clean
        .as_bytes()
        .chunks(2)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn cert_fingerprint_from_pem(cert_pem: &str) -> Result<String> {
    let mut cursor = Cursor::new(cert_pem.as_bytes());
    let certs: Vec<_> = rustls_pemfile::certs(&mut cursor)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to parse PEM certificate")?;
    let first = certs
        .first()
        .context("Certificate PEM file contains no certificates")?;
    Ok(hex::encode(Sha256::digest(first.as_ref())))
}

pub fn load_or_generate_tls(dir: &Path, hostname: &str) -> Result<TlsMaterial> {
    let cert_path = dir.join("server.crt");
    let key_path = dir.join("server.key");

    if cert_path.exists() && key_path.exists() {
        let cert_pem = fs::read_to_string(&cert_path)
            .with_context(|| format!("Failed to read TLS certificate at {:?}", cert_path))?;
        let key_pem = fs::read_to_string(&key_path)
            .with_context(|| format!("Failed to read TLS private key at {:?}", key_path))?;

        if !key_pem.trim().is_empty() {
            let fingerprint_sha256 = cert_fingerprint_from_pem(&cert_pem)?;
            return Ok(TlsMaterial {
                cert_pem,
                key_pem,
                fingerprint_sha256,
            });
        }
    }

    let clean_host = hostname
        .trim()
        .trim_end_matches('.')
        .strip_suffix(".local")
        .or_else(|| hostname.trim().trim_end_matches('.').strip_suffix(".LOCAL"))
        .unwrap_or_else(|| hostname.trim().trim_end_matches('.'));

    let mut seen = HashSet::new();
    let mut sans = Vec::new();
    let mut push_san = |val: String| {
        let trimmed = val.trim().to_string();
        if !trimmed.is_empty() && seen.insert(trimmed.to_ascii_lowercase()) {
            sans.push(trimmed);
        }
    };

    push_san("localhost".to_string());
    push_san("127.0.0.1".to_string());
    push_san("::1".to_string());
    push_san("ntx-agent".to_string());
    if !clean_host.is_empty() {
        push_san(clean_host.to_string());
        push_san(format!("{}.local", clean_host));
    }

    let key_pair = KeyPair::generate().context("Failed to generate ECDSA TLS key pair")?;
    let mut params = CertificateParams::new(sans)
        .context("Failed to create X.509 certificate parameters")?;
    params
        .distinguished_name
        .push(DnType::CommonName, "ntx-agent");

    let cert = params
        .self_signed(&key_pair)
        .context("Failed to self-sign TLS certificate")?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    let fingerprint_sha256 = hex::encode(Sha256::digest(cert.der().as_ref()));

    write_file_atomic(&cert_path, cert_pem.as_bytes(), 0o644)
        .with_context(|| format!("Failed to write TLS certificate to {:?}", cert_path))?;
    write_file_atomic(&key_path, key_pem.as_bytes(), 0o600)
        .with_context(|| format!("Failed to write TLS private key to {:?}", key_path))?;

    Ok(TlsMaterial {
        cert_pem,
        key_pem,
        fingerprint_sha256,
    })
}

pub fn server_tls_config(material: &TlsMaterial) -> Result<ServerTlsConfig> {
    let identity = Identity::from_pem(&material.cert_pem, &material.key_pem);
    Ok(ServerTlsConfig::new().identity(identity))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tls_generate_and_reload() {
        let temp_dir =
            std::env::temp_dir().join(format!("ntx_test_tls_{}", uuid::Uuid::new_v4().simple()));

        let mat1 = load_or_generate_tls(&temp_dir, "rpi5-node.local").unwrap();
        assert_eq!(mat1.fingerprint_sha256.len(), 64);
        assert!(mat1.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(mat1.key_pem.contains("BEGIN PRIVATE KEY"));

        let parsed_fp = cert_fingerprint_from_pem(&mat1.cert_pem).unwrap();
        assert_eq!(parsed_fp, mat1.fingerprint_sha256);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let key_mode = fs::metadata(temp_dir.join("server.key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(key_mode, 0o600);
        }

        // Reloading from the same directory yields identical fingerprint & cert
        let mat2 = load_or_generate_tls(&temp_dir, "rpi5-node.local").unwrap();
        assert_eq!(mat1.fingerprint_sha256, mat2.fingerprint_sha256);
        assert_eq!(mat1.cert_pem, mat2.cert_pem);
        assert_eq!(mat1.key_pem, mat2.key_pem);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_format_fingerprint() {
        let formatted = format_fingerprint("a1b2c3d4");
        assert_eq!(formatted, "A1:B2:C3:D4");
        assert_eq!(format_fingerprint("AA:bb:cc"), "AA:BB:CC");
    }
}
