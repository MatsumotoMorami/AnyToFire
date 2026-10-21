// Read-only HTTPS probe; no access code or score is sent.
fn main() {
    let tls = ureq::native_tls::TlsConnector::new().expect("Windows TLS connector");
    let agent = ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(tls))
        .build();
    match agent.get("https://services.firenet.iidx.pro/")
        .timeout(std::time::Duration::from_secs(10)).call()
    {
        Ok(response) => println!("HTTPS connected: HTTP {}", response.status()),
        Err(ureq::Error::Status(status, _)) => println!("HTTPS connected: HTTP {status}"),
        Err(error) => panic!("HTTPS connection failed: {error}"),
    }
}
