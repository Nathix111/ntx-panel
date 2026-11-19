use hyper_util::rt::TokioIo;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as RustlsError, SignatureScheme};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

pub fn normalize_fingerprint(fp: &str) -> String {
    fp.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_lowercase()
}

pub fn format_fingerprint(fp: &str) -> String {
    let clean = normalize_fingerprint(fp).to_ascii_uppercase();
    clean
        .as_bytes()
        .chunks(2)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(":")
}

#[derive(Debug, thiserror::Error)]
pub enum TlsConnectError {
    #[error("L'empreinte TLS du serveur a changé (attendu: {expected}, reçu: {actual})")]
    FingerprintMismatch { expected: String, actual: String },
    #[error("{0}")]
    Other(String),
}

#[derive(Debug)]
struct TofuCertVerifier {
    expected_fingerprint: Option<String>,
    observed_fingerprint: Arc<Mutex<Option<String>>>,
    supported_algs: WebPkiSupportedAlgorithms,
}

impl TofuCertVerifier {
    fn new(
        expected_fingerprint: Option<String>,
        observed_fingerprint: Arc<Mutex<Option<String>>>,
    ) -> Self {
        let supported_algs = rustls::crypto::ring::default_provider().signature_verification_algorithms;
        Self {
            expected_fingerprint,
            observed_fingerprint,
            supported_algs,
        }
    }
}

impl ServerCertVerifier for TofuCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        let actual = hex::encode(Sha256::digest(end_entity.as_ref()));
        if let Ok(mut guard) = self.observed_fingerprint.lock() {
            *guard = Some(actual.clone());
        }

        if let Some(ref expected) = self.expected_fingerprint {
            let matches = expected.len() == actual.len()
                && bool::from(expected.as_bytes().ct_eq(actual.as_bytes()));
            if !matches {
                return Err(RustlsError::General(format!(
                    "TLS_FINGERPRINT_MISMATCH:{expected}:{actual}"
                )));
            }
        }

        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls12_signature(message, cert, dss, &self.supported_algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls13_signature(message, cert, dss, &self.supported_algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_algs.supported_schemes()
    }
}

pub async fn connect_tls_channel(
    endpoint_url: &str,
    expected_fingerprint: Option<String>,
) -> Result<(Channel, String), TlsConnectError> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let expected_norm = expected_fingerprint
        .map(|s| normalize_fingerprint(&s))
        .filter(|s| !s.is_empty());

    let uri: Uri = endpoint_url
        .parse()
        .map_err(|e| TlsConnectError::Other(format!("URI invalide '{}': {}", endpoint_url, e)))?;

    let raw_host = uri.host().ok_or_else(|| {
        TlsConnectError::Other(format!("Hôte manquant dans l'URI '{}'", endpoint_url))
    })?;
    let authority = uri
        .authority()
        .ok_or_else(|| {
            TlsConnectError::Other(format!("Autorité manquante dans l'URI '{}'", endpoint_url))
        })?
        .as_str()
        .to_string();
    let connect_host = raw_host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = uri.port_u16().unwrap_or(9850);

    let observed_fingerprint = Arc::new(Mutex::new(None));
    let verifier = Arc::new(TofuCertVerifier::new(
        expected_norm.clone(),
        Arc::clone(&observed_fingerprint),
    ));

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut client_config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| TlsConnectError::Other(format!("Configuration TLS invalide: {}", e)))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    client_config.alpn_protocols = vec![b"h2".to_vec()];
    let client_config = Arc::new(client_config);

    // Tonic's internal Connector rejects https:// URIs unless Endpoint::tls_config is set,
    // even when our custom service_fn connector performs the rustls handshake itself.
    let transport_uri = format!("http://{}", authority);
    let endpoint = Endpoint::from_shared(transport_uri)
        .map_err(|e| TlsConnectError::Other(format!("Endpoint gRPC invalide: {}", e)))?
        .origin(uri)
        .connect_timeout(Duration::from_secs(5))
        .http2_keep_alive_interval(Duration::from_secs(15))
        .keep_alive_timeout(Duration::from_secs(10))
        .keep_alive_while_idle(true);

    let connector = service_fn(move |_uri: Uri| {
        let host = connect_host.clone();
        let cfg = Arc::clone(&client_config);
        async move {
            let tcp = TcpStream::connect((host.as_str(), port)).await?;
            let _ = tcp.set_nodelay(true);
            let server_name = ServerName::try_from("ntx-agent")
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            let tls_stream = TlsConnector::from(cfg).connect(server_name, tcp).await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(TokioIo::new(tls_stream))
        }
    });

    match endpoint.connect_with_connector(connector).await {
        Ok(channel) => {
            let actual = observed_fingerprint
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .ok_or_else(|| {
                    TlsConnectError::Other("Empreinte TLS serveur non observée".to_string())
                })?;
            Ok((channel, actual))
        }
        Err(err) => {
            let observed = observed_fingerprint.lock().ok().and_then(|g| g.clone());
            if let (Some(expected), Some(actual)) = (expected_norm, observed) {
                if expected != actual {
                    return Err(TlsConnectError::FingerprintMismatch { expected, actual });
                }
            }
            Err(TlsConnectError::Other(format!(
                "Échec de connexion TLS vers {}: {}",
                endpoint_url, err
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, DnType, KeyPair};
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::transport::{Identity, Server, ServerTlsConfig};

    fn make_self_signed_cert() -> (String, String, String) {
        let kp = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "ntx-agent".to_string(),
        ])
        .unwrap();
        params.distinguished_name.push(DnType::CommonName, "ntx-agent");
        let cert = params.self_signed(&kp).unwrap();
        let fp = hex::encode(Sha256::digest(cert.der().as_ref()));
        (cert.pem(), kp.serialize_pem(), fp)
    }

    #[test]
    fn test_fingerprint_formatting_and_normalization() {
        let norm = normalize_fingerprint("AA:bb:CC:dd");
        assert_eq!(norm, "aabbccdd");
        assert_eq!(format_fingerprint(&norm), "AA:BB:CC:DD");
    }

    #[tokio::test]
    async fn test_tofu_pin_accept_and_mismatch() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (cert_pem, key_pem, fp) = make_self_signed_cert();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let server_tls = ServerTlsConfig::new().identity(Identity::from_pem(&cert_pem, &key_pem));

        // Minimal dummy router via tonic Server
        let (tx, _rx) = tokio::sync::mpsc::channel::<()>(1);
        drop(tx);

        tokio::spawn(async move {
            use ntx_proto::ntx_agent_server::{NtxAgent, NtxAgentServer};
            struct Dummy;
            #[tonic::async_trait]
            impl NtxAgent for Dummy {
                async fn get_auth_challenge(
                    &self,
                    _: tonic::Request<ntx_proto::AuthChallengeRequest>,
                ) -> Result<tonic::Response<ntx_proto::AuthChallengeResponse>, tonic::Status>
                {
                    unimplemented!()
                }
                async fn authenticate(
                    &self,
                    _: tonic::Request<ntx_proto::AuthRequest>,
                ) -> Result<tonic::Response<ntx_proto::AuthResponse>, tonic::Status> {
                    unimplemented!()
                }
                async fn get_system_info(
                    &self,
                    _: tonic::Request<ntx_proto::Empty>,
                ) -> Result<tonic::Response<ntx_proto::SystemInfo>, tonic::Status> {
                    unimplemented!()
                }
                type StreamMetricsStream =
                    tokio_stream::wrappers::ReceiverStream<Result<ntx_proto::SystemMetrics, tonic::Status>>;
                async fn stream_metrics(
                    &self,
                    _: tonic::Request<ntx_proto::MetricsRequest>,
                ) -> Result<tonic::Response<Self::StreamMetricsStream>, tonic::Status> {
                    unimplemented!()
                }
                type OpenTerminalStream =
                    tokio_stream::wrappers::ReceiverStream<Result<ntx_proto::TerminalOutput, tonic::Status>>;
                async fn open_terminal(
                    &self,
                    _: tonic::Request<tonic::Streaming<ntx_proto::TerminalInput>>,
                ) -> Result<tonic::Response<Self::OpenTerminalStream>, tonic::Status> {
                    unimplemented!()
                }
                async fn ping(
                    &self,
                    _: tonic::Request<ntx_proto::Empty>,
                ) -> Result<tonic::Response<ntx_proto::PingResponse>, tonic::Status> {
                    Ok(tonic::Response::new(ntx_proto::PingResponse {
                        hostname: "tls-test".to_string(),
                        uptime_seconds: 1,
                        version: "0.2.0".to_string(),
                    }))
                }
                async fn list_processes(
                    &self,
                    _: tonic::Request<ntx_proto::ListProcessesRequest>,
                ) -> Result<tonic::Response<ntx_proto::ListProcessesResponse>, tonic::Status>
                {
                    unimplemented!()
                }
                async fn kill_process(
                    &self,
                    _: tonic::Request<ntx_proto::KillProcessRequest>,
                ) -> Result<tonic::Response<ntx_proto::KillProcessResponse>, tonic::Status> {
                    unimplemented!()
                }
                async fn list_services(
                    &self,
                    _: tonic::Request<ntx_proto::ListServicesRequest>,
                ) -> Result<tonic::Response<ntx_proto::ListServicesResponse>, tonic::Status> {
                    unimplemented!()
                }
                async fn control_service(
                    &self,
                    _: tonic::Request<ntx_proto::ControlServiceRequest>,
                ) -> Result<tonic::Response<ntx_proto::ControlServiceResponse>, tonic::Status>
                {
                    unimplemented!()
                }
            }

            Server::builder()
                .tls_config(server_tls)
                .unwrap()
                .add_service(NtxAgentServer::new(Dummy))
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        let endpoint = format!("https://{}", addr);

        // 1. First connection (TOFU, no expected fingerprint) succeeds and observes fingerprint
        let (_chan1, observed1) = connect_tls_channel(&endpoint, None).await.unwrap();
        assert_eq!(observed1, fp);

        // 2. Pinned connection with matching fingerprint succeeds
        let (_chan2, observed2) = connect_tls_channel(&endpoint, Some(fp.clone())).await.unwrap();
        assert_eq!(observed2, fp);

        // 3. Pinned connection with mismatched fingerprint fails with FingerprintMismatch
        let wrong_fp = "00".repeat(32);
        let err = connect_tls_channel(&endpoint, Some(wrong_fp.clone()))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            TlsConnectError::FingerprintMismatch { expected, actual } if expected == wrong_fp && actual == fp
        ));

        let _ = shutdown_tx.send(());
    }
}
