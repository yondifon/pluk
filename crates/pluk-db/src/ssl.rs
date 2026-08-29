use crate::error::DriverError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SslMode {
    Disable,
    Require,
    VerifyCa,
    VerifyFull,
}

impl SslMode {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Self::parse(s)
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "disable" => Some(Self::Disable),
            "require" => Some(Self::Require),
            "verify-ca" => Some(Self::VerifyCa),
            "verify-full" => Some(Self::VerifyFull),
            _ => None,
        }
    }
    pub fn as_str(&self) -> &str {
        match self {
            Self::Disable => "disable",
            Self::Require => "require",
            Self::VerifyCa => "verify-ca",
            Self::VerifyFull => "verify-full",
        }
    }
    pub fn verifies(&self) -> bool {
        matches!(self, Self::VerifyCa | Self::VerifyFull)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SslConfig {
    pub mode: Option<SslMode>,
    pub ca: Option<Vec<u8>>,
    pub cert: Option<Vec<u8>>,
    pub key: Option<Vec<u8>>,
    pub reject_unauthorized: bool,
}

impl SslConfig {
    pub fn disabled() -> Self {
        Self {
            mode: Some(SslMode::Disable),
            ..Default::default()
        }
    }
    pub fn is_disabled(&self) -> bool {
        matches!(self.mode, Some(SslMode::Disable)) || self.mode.is_none()
    }
}

pub fn build_ssl_config(
    use_ssl: bool,
    ssl_mode: Option<&str>,
    ca_path: Option<&str>,
    cert_path: Option<&str>,
    key_path: Option<&str>,
) -> Result<Option<SslConfig>, DriverError> {
    let mode_str = ssl_mode.unwrap_or("");
    if !use_ssl || mode_str == "disable" {
        return Ok(None);
    }
    let mode = SslMode::from_str(mode_str).unwrap_or(SslMode::Require);

    let reject_unauthorized = matches!(mode, SslMode::VerifyCa | SslMode::VerifyFull);

    let ca = if let Some(p) = ca_path.filter(|p| !p.is_empty()) {
        Some(std::fs::read(p).map_err(|e| DriverError::Ssl(format!("ca read error: {e}")))?)
    } else {
        None
    };
    let cert = if let Some(p) = cert_path.filter(|p| !p.is_empty()) {
        Some(std::fs::read(p).map_err(|e| DriverError::Ssl(format!("cert read error: {e}")))?)
    } else {
        None
    };
    let key = if let Some(p) = key_path.filter(|p| !p.is_empty()) {
        Some(std::fs::read(p).map_err(|e| DriverError::Ssl(format!("key read error: {e}")))?)
    } else {
        None
    };

    // Verify-ca / verify-full must have CA to actually verify — mirroring pg's
    // rejectUnauthorized semantics. We enforce file presence already; TLS connector
    // will enforce verification at connect time.
    Ok(Some(SslConfig {
        mode: Some(mode),
        ca,
        cert,
        key,
        reject_unauthorized,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ssl_mode_mapping() {
        assert_eq!(SslMode::from_str("disable"), Some(SslMode::Disable));
        assert_eq!(SslMode::from_str("require"), Some(SslMode::Require));
        assert_eq!(SslMode::from_str("verify-ca"), Some(SslMode::VerifyCa));
        assert_eq!(SslMode::from_str("verify-full"), Some(SslMode::VerifyFull));
        assert!(SslMode::VerifyCa.verifies());
        assert!(SslMode::VerifyFull.verifies());
        assert!(!SslMode::Require.verifies());
        assert!(!SslMode::Disable.verifies());
    }
    #[test]
    fn build_disabled_when_no_ssl() {
        assert!(
            build_ssl_config(false, Some("verify-ca"), None, None, None)
                .unwrap()
                .is_none()
        );
        assert!(
            build_ssl_config(true, Some("disable"), None, None, None)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn build_require_has_no_verification() {
        let c = build_ssl_config(true, Some("require"), None, None, None)
            .unwrap()
            .unwrap();
        assert_eq!(c.mode, Some(SslMode::Require));
        assert!(!c.reject_unauthorized);
    }
    #[test]
    fn build_verify_ca_enforces_verification() {
        let c = build_ssl_config(true, Some("verify-ca"), None, None, None)
            .unwrap()
            .unwrap();
        assert!(c.reject_unauthorized);
    }
}
