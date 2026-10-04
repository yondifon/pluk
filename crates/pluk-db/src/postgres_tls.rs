use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};

use crate::error::DriverError;
use crate::ssl::SslConfig;

pub(super) fn client_config(ssl: &SslConfig) -> Result<ClientConfig, DriverError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let ca = ssl
        .ca
        .as_deref()
        .map(|pem| certificates(pem).map_err(|e| DriverError::Ssl(format!("ca read error: {e}"))))
        .transpose()?;
    let verifier: Arc<dyn ServerCertVerifier> = if ssl.reject_unauthorized {
        Arc::new(
            rustls_platform_verifier::Verifier::new_with_extra_roots(
                ca.unwrap_or_default(),
                provider.clone(),
            )
            .map_err(|e| DriverError::Ssl(e.to_string()))?,
        )
    } else {
        Arc::new(NoCertificateVerification(provider.clone()))
    };
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| DriverError::Ssl(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier);
    if let (Some(cert), Some(key)) = (&ssl.cert, &ssl.key) {
        let certs =
            certificates(cert).map_err(|e| DriverError::Ssl(format!("cert/key error: {e}")))?;
        let key = rustls_pemfile::pkcs8_private_keys(&mut key.as_slice())
            .next()
            .transpose()
            .map_err(|e| DriverError::Ssl(format!("cert/key error: {e}")))?
            .ok_or_else(|| DriverError::Ssl("cert/key error: no PKCS#8 private key".into()))?;
        builder
            .with_client_auth_cert(certs, key.into())
            .map_err(|e| DriverError::Ssl(format!("cert/key error: {e}")))
    } else {
        Ok(builder.with_no_client_auth())
    }
}

fn certificates(mut pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, std::io::Error> {
    let certs = rustls_pemfile::certs(&mut pem).collect::<Result<Vec<_>, _>>()?;
    if certs.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "no PEM certificates",
        ));
    }
    Ok(certs)
}

#[derive(Debug)]
struct NoCertificateVerification(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for NoCertificateVerification {
    // `require` encrypts without authenticating the certificate or hostname.
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssl::build_ssl_config;
    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use rustls::{ClientConnection, ServerConfig, ServerConnection};

    fn handshake(config: ClientConfig, server: ServerConfig) -> Result<(), rustls::Error> {
        let mut client = ClientConnection::new(
            Arc::new(config),
            ServerName::try_from("postgres.example").unwrap().to_owned(),
        )
        .unwrap();
        let mut server = ServerConnection::new(Arc::new(server)).unwrap();
        while client.is_handshaking() || server.is_handshaking() {
            let mut bytes = Vec::new();
            client.write_tls(&mut bytes).unwrap();
            server.read_tls(&mut bytes.as_slice()).unwrap();
            server.process_new_packets()?;
            bytes.clear();
            server.write_tls(&mut bytes).unwrap();
            client.read_tls(&mut bytes.as_slice()).unwrap();
            client.process_new_packets()?;
        }
        Ok(())
    }

    fn server(identity: &CertifiedKey<rcgen::KeyPair>) -> ServerConfig {
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![identity.cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(identity.signing_key.serialize_der())
                    .into(),
            )
            .unwrap()
    }

    #[test]
    fn require_accepts_untrusted_certificate_with_wrong_hostname() {
        let identity = generate_simple_self_signed(vec!["other.example".into()]).unwrap();
        let ssl = build_ssl_config(true, Some("require"), None, None, None)
            .unwrap()
            .unwrap();
        handshake(client_config(&ssl).unwrap(), server(&identity)).unwrap();
    }

    #[test]
    fn verifying_modes_reject_untrusted_certificates_and_wrong_hostnames() {
        let matching = generate_simple_self_signed(vec!["postgres.example".into()]).unwrap();
        let wrong = generate_simple_self_signed(vec!["other.example".into()]).unwrap();
        for mode in ["verify-ca", "verify-full"] {
            let mut ssl = build_ssl_config(true, Some(mode), None, None, None)
                .unwrap()
                .unwrap();
            assert!(handshake(client_config(&ssl).unwrap(), server(&matching)).is_err());
            ssl.ca = Some(matching.cert.pem().into_bytes());
            handshake(client_config(&ssl).unwrap(), server(&matching)).unwrap();
            ssl.ca = Some(wrong.cert.pem().into_bytes());
            assert!(handshake(client_config(&ssl).unwrap(), server(&wrong)).is_err());
        }
    }

    #[test]
    fn client_identity_is_sent_when_the_server_requires_it() {
        let identity = generate_simple_self_signed(vec!["postgres.example".into()]).unwrap();
        let client = generate_simple_self_signed(vec!["client.example".into()]).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(client.cert.der().clone()).unwrap();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            provider.clone(),
        )
        .build()
        .unwrap();
        let server = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_client_cert_verifier(verifier)
            .with_single_cert(
                vec![identity.cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(identity.signing_key.serialize_der())
                    .into(),
            )
            .unwrap();
        let mut ssl = build_ssl_config(true, Some("require"), None, None, None)
            .unwrap()
            .unwrap();
        assert!(handshake(client_config(&ssl).unwrap(), server.clone()).is_err());
        ssl.cert = Some(client.cert.pem().into_bytes());
        ssl.key = Some(client.signing_key.serialize_pem().into_bytes());
        handshake(client_config(&ssl).unwrap(), server).unwrap();
    }
}
