//! Bounded observations from the certificate workers, never a second issuer.
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use x509_parser::prelude::{FromDer, X509Certificate};

#[derive(Debug, Default)]
pub(crate) struct Status {
    states: Mutex<HashMap<String, Value>>,
    pub retry: tokio::sync::Notify,
}
impl Status {
    pub(crate) fn record(&self, domain: &str, state: &str, next: Option<i64>) {
        let mut states = self.states.lock();
        if states.len() >= 4096 && !states.contains_key(domain) {
            return;
        }
        states.insert(
            domain.into(),
            json!({"domain":domain,"state":state,
            "updated_at":crate::relay::now(),"next_attempt":next}),
        );
    }
    pub(crate) fn snapshot(&self) -> Vec<Value> {
        self.states.lock().values().cloned().collect()
    }
}

pub(crate) fn certificate(name: &str, der: &rustls::pki_types::CertificateDer<'_>) -> Value {
    let validity = X509Certificate::from_der(der.as_ref()).ok().map(|(_, cert)| {
        json!({"not_before":cert.validity().not_before.timestamp(),"not_after":cert.validity().not_after.timestamp()})
    });
    json!({"name":name,"fingerprint":felucca_transport::tls::fingerprint_of(der),"validity":validity})
}
