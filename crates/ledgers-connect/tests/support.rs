//! A recorded vendor: an HTTP port that answers from fixtures and records every
//! request, so tests never reach a vendor. Routes match on the URL and on the
//! request body (GraphQL variables carry the month).

#![allow(dead_code)]
#![allow(clippy::must_use_candidate, clippy::missing_panics_doc)]

use bytes::Bytes;
use cratefield_core::{HttpClient, HttpError};
use ledgers_connect::http::{Pause, VendorHttp};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One recorded answer: the URL needle, the body needle, the status and body.
pub struct Route {
    pub url: &'static str,
    pub body: &'static str,
    pub status: u16,
    pub answer: String,
}

/// Fixture-backed HTTP client.
#[derive(Default)]
pub struct Recorded {
    routes: Vec<Route>,
    queue: Mutex<VecDeque<(u16, String)>>,
    pub requests: Mutex<Vec<(String, String, String)>>,
    /// The body of each request, in order (empty for a GET).
    pub bodies: Mutex<Vec<String>>,
}

impl Recorded {
    pub fn new(routes: Vec<Route>) -> Arc<Self> {
        Arc::new(Self {
            routes,
            queue: Mutex::new(VecDeque::new()),
            requests: Mutex::new(Vec::new()),
            bodies: Mutex::new(Vec::new()),
        })
    }

    /// Answers the next calls with these statuses and bodies, in order, before
    /// the routes are consulted.
    pub fn script(answers: Vec<(u16, &str)>) -> Arc<Self> {
        Arc::new(Self {
            routes: Vec::new(),
            queue: Mutex::new(
                answers
                    .into_iter()
                    .map(|(s, b)| (s, b.to_owned()))
                    .collect(),
            ),
            requests: Mutex::new(Vec::new()),
            bodies: Mutex::new(Vec::new()),
        })
    }

    pub fn calls(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl HttpClient for Recorded {
    async fn send(
        &self,
        request: http::Request<Bytes>,
    ) -> Result<http::Response<Bytes>, HttpError> {
        let uri = request.uri().to_string();
        let body = String::from_utf8_lossy(request.body()).to_string();
        let auth = request
            .headers()
            .get("authorization")
            .or_else(|| request.headers().get("x-api-key"))
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        self.requests
            .lock()
            .unwrap()
            .push((request.method().to_string(), uri.clone(), auth));
        self.bodies.lock().unwrap().push(body.clone());
        if let Some((status, answer)) = self.queue.lock().unwrap().pop_front() {
            return Ok(response(status, answer));
        }
        for route in &self.routes {
            if uri.contains(route.url) && body.contains(route.body) {
                return Ok(response(route.status, route.answer.clone()));
            }
        }
        Ok(response(404, "{\"error\":\"no fixture\"}".to_owned()))
    }
}

fn response(status: u16, body: String) -> http::Response<Bytes> {
    http::Response::builder()
        .status(status)
        .body(Bytes::from(body))
        .expect("a valid response")
}

/// Records the waits instead of sleeping.
#[derive(Default)]
pub struct RecordedPause(pub Mutex<Vec<Duration>>);

#[async_trait::async_trait]
impl Pause for RecordedPause {
    async fn pause(&self, delay: Duration) {
        self.0.lock().unwrap().push(delay);
    }
}

pub fn vendor(client: Arc<Recorded>) -> (VendorHttp, Arc<RecordedPause>) {
    let pause = Arc::new(RecordedPause::default());
    (VendorHttp::new(client, pause.clone()), pause)
}

pub const FIX_CF_HISTORY: &str = include_str!("../fixtures/cloudflare_billing_history.json");
pub const FIX_CF_USAGE_AUG: &str = include_str!("../fixtures/cloudflare_usage_2026-08.json");
pub const FIX_CF_USAGE_JUL_ERR: &str =
    include_str!("../fixtures/cloudflare_usage_2026-07_error.json");
pub const FIX_ANT_COST: &str = include_str!("../fixtures/anthropic_cost_2026-07.json");
pub const FIX_ANT_USAGE: &str = include_str!("../fixtures/anthropic_usage_2026-07.json");
pub const FIX_PS_INVOICES: &str = include_str!("../fixtures/planetscale_invoices.json");

/// The routes every connector test uses: the three vendors' recorded answers.
pub fn all_routes() -> Vec<Route> {
    vec![
        Route {
            url: "/billing/history",
            body: "",
            status: 200,
            answer: FIX_CF_HISTORY.to_owned(),
        },
        Route {
            url: "/graphql",
            body: "2026-08-01",
            status: 200,
            answer: FIX_CF_USAGE_AUG.to_owned(),
        },
        Route {
            url: "/graphql",
            body: "2026-07-01",
            status: 200,
            answer: FIX_CF_USAGE_JUL_ERR.to_owned(),
        },
        Route {
            url: "/organizations/cost_report",
            body: "",
            status: 200,
            answer: FIX_ANT_COST.to_owned(),
        },
        Route {
            url: "/organizations/usage_report/messages",
            body: "",
            status: 200,
            answer: FIX_ANT_USAGE.to_owned(),
        },
        Route {
            url: "/invoices",
            body: "",
            status: 200,
            answer: FIX_PS_INVOICES.to_owned(),
        },
    ]
}
