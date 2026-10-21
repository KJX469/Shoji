//! Site information for the side panel.
//!
//! WebView2 does not expose the certificate of a successful navigation, so details are read
//! with a separate TLS handshake — but only when the user opens the Site panel, never on
//! ordinary navigation. Results are cached per host for the session.

use crate::BrowserEvent;
use shoji_core::ipc::SecurityInfo;
use std::collections::HashMap;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::{Arc, Mutex};
use winit::event_loop::EventLoopProxy;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Only plain DNS names / IPv4 literals are ever passed to the shell.
fn is_safe_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

#[derive(Clone, Default)]
pub struct CertificateCache {
    // `None` marks a lookup in flight.
    cache: Arc<Mutex<HashMap<String, Option<SecurityInfo>>>>,
}

impl CertificateCache {
    /// Describe `url` for the Site panel. With `fetch`, start a background certificate
    /// lookup for HTTPS hosts not yet cached; `CertificateReady` fires when it completes.
    pub fn describe(
        &self,
        url: &str,
        persistent: bool,
        fetch: bool,
        proxy: &EventLoopProxy<BrowserEvent>,
    ) -> SecurityInfo {
        if url.starts_with(shoji_core::INTERNAL_SCHEME) || url == "about:blank" || url.is_empty() {
            return SecurityInfo {
                host: url.to_string(),
                is_internal: true,
                is_secure: true,
                ..Default::default()
            };
        }
        let host = crate::extract_host(url).unwrap_or_default();
        let is_secure = url.starts_with("https://");
        let base = SecurityInfo {
            host: host.clone(),
            is_secure,
            is_persistent: persistent,
            ..Default::default()
        };
        if !is_secure || !is_safe_host(&host) {
            return base;
        }
        let Ok(mut cache) = self.cache.lock() else {
            return base;
        };
        match cache.get(&host) {
            Some(Some(info)) => SecurityInfo {
                is_persistent: persistent,
                ..info.clone()
            },
            Some(None) => base,
            None if fetch => {
                cache.insert(host.clone(), None);
                let cache = self.cache.clone();
                let proxy = proxy.clone();
                std::thread::spawn(move || {
                    let info = fetch_certificate(&host);
                    if let Ok(mut c) = cache.lock() {
                        match info {
                            Some(info) => c.insert(host.clone(), Some(info)),
                            None => c.remove(&host), // allow a retry next time
                        };
                    }
                    let _ = proxy.send_event(BrowserEvent::CertificateReady(host));
                });
                base
            }
            None => base,
        }
    }
}

fn powershell(script: &str) -> Command {
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

const HANDSHAKE: &str = "$t = New-Object System.Net.Sockets.TcpClient($h, 443); \
$s = New-Object System.Net.Security.SslStream($t.GetStream(), $false, ({$true})); \
$s.AuthenticateAsClient($h); \
$c = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2($s.RemoteCertificate);";

/// Read the certificate presented by `host:443`.
fn fetch_certificate(host: &str) -> Option<SecurityInfo> {
    let script = format!(
        "$h='{host}'; try {{ {HANDSHAKE} \
         Write-Output ('CERT|' + $c.Subject + '|' + $c.Issuer + '|' + $c.NotBefore.ToString('yyyy-MM-dd') + '|' + $c.NotAfter.ToString('yyyy-MM-dd') + '|' + $c.Thumbprint + '|' + $c.SerialNumber + '|' + $c.SignatureAlgorithm.FriendlyName); \
         $t.Close() }} catch {{}}"
    );
    let output = powershell(&script).output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("CERT|"))?;
    let p: Vec<&str> = line.split('|').collect();
    (p.len() >= 7).then(|| SecurityInfo {
        host: host.to_string(),
        is_secure: true,
        has_certificate: true,
        subject: p[0].to_string(),
        issuer: p[1].to_string(),
        valid_from: p[2].to_string(),
        valid_to: p[3].to_string(),
        thumbprint: p[4].to_string(),
        serial_number: p[5].to_string(),
        signature_algorithm: p[6].to_string(),
        ..Default::default()
    })
}

/// Open the native Windows certificate viewer for `host`.
pub fn open_certificate_dialog(host: &str) {
    if !is_safe_host(host) {
        return;
    }
    let script = format!(
        "Add-Type -AssemblyName System.Security; $h='{host}'; try {{ {HANDSHAKE} \
         [System.Security.Cryptography.X509Certificates.X509Certificate2UI]::DisplayCertificate($c); $t.Close() }} catch {{}}"
    );
    let _ = powershell(&script).spawn();
}

#[cfg(test)]
mod tests {
    use super::is_safe_host;

    #[test]
    fn rejects_shell_metacharacters() {
        assert!(is_safe_host("example.com"));
        assert!(is_safe_host("a-b.c0m"));
        assert!(!is_safe_host("a.com'; calc; '"));
        assert!(!is_safe_host("a.com$(x)"));
        assert!(!is_safe_host(""));
    }
}
