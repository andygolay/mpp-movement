use crate::proxy::service::{Service, ServiceBuilder};

/// Create a Firecrawl service configuration.
///
/// Injects `Authorization: Bearer` header for upstream authentication.
///
/// # Example
///
/// ```
/// use mpp::proxy::service::{Endpoint, PaidEndpoint, ServiceBuilder};
/// use mpp::proxy::services::firecrawl;
///
/// let svc = firecrawl::service("fc-...", |r| {
///     r.route("POST /v1/scrape", Endpoint::Paid(PaidEndpoint {
///         intent: "charge".into(),
///         amount: "0.01".into(),
///         unit_type: None,
///         description: Some("Scrape a URL".into()),
///     }))
/// });
///
/// assert_eq!(svc.id, "firecrawl");
/// ```
pub fn service(api_key: &str, configure: impl FnOnce(ServiceBuilder) -> ServiceBuilder) -> Service {
    configure(Service::new("firecrawl", "https://api.firecrawl.dev").bearer(api_key)).build()
}
