use std::{
    collections::{BTreeMap, VecDeque},
    time::{Duration, Instant, SystemTime},
};

use crate::{host::HostHealth, metrics::EndpointWindow, thresholds};

#[derive(Clone, Debug)]
pub struct Alert {
    pub check: String,
    pub observed: String,
    pub threshold: String,
    pub fired_at: SystemTime,
}

#[derive(Clone, Debug)]
pub enum AlertTransition {
    Fired(Alert),
    Recovered(Alert),
}

#[derive(Default)]
pub struct AlertEngine {
    active: BTreeMap<String, Alert>,
    recent: VecDeque<(SystemTime, String)>,
    scrape_failures: u32,
    ready_failed_since: Option<Instant>,
}

pub struct AlertInput<'a> {
    pub now: Instant,
    pub scrape_ok: bool,
    pub ready_ok: bool,
    pub endpoints: &'a BTreeMap<String, EndpointWindow>,
    pub host: &'a HostHealth,
}

impl AlertEngine {
    pub fn evaluate(&mut self, input: AlertInput<'_>) -> Vec<AlertTransition> {
        self.scrape_failures = if input.scrape_ok {
            0
        } else {
            self.scrape_failures.saturating_add(1)
        };
        self.ready_failed_since = if input.ready_ok {
            None
        } else {
            Some(self.ready_failed_since.unwrap_or(input.now))
        };

        let mut conditions = BTreeMap::new();
        conditions.insert(
            "scrape_failure".to_string(),
            (
                self.scrape_failures >= thresholds::SCRAPE_FAILURE_TICKS,
                format!("{} consecutive failed ticks", self.scrape_failures),
                format!("{} ticks", thresholds::SCRAPE_FAILURE_TICKS),
            ),
        );
        let ready_failed_for = self
            .ready_failed_since
            .map(|since| input.now.duration_since(since))
            .unwrap_or_default();
        conditions.insert(
            "ready".to_string(),
            (
                !input.ready_ok
                    && ready_failed_for >= Duration::from_secs(thresholds::READY_FAILURE_SECONDS),
                format!("non-200 for {}s", ready_failed_for.as_secs()),
                format!("{}s", thresholds::READY_FAILURE_SECONDS),
            ),
        );

        for endpoint in crate::metrics::ENDPOINTS {
            let window = input.endpoints.get(endpoint).cloned().unwrap_or_default();
            conditions.insert(
                format!("{endpoint}_5xx"),
                (
                    window.requests >= thresholds::HTTP_5XX_MIN_REQUESTS
                        && window.error_ratio > thresholds::HTTP_5XX_RATIO,
                    format!(
                        "{:.1}% over {:.0} requests",
                        window.error_ratio * 100.0,
                        window.requests
                    ),
                    format!(
                        "> {:.0}% over 5m, min {:.0}",
                        thresholds::HTTP_5XX_RATIO * 100.0,
                        thresholds::HTTP_5XX_MIN_REQUESTS
                    ),
                ),
            );
            let (latency_check, latency_label, latency_threshold) = match endpoint {
                "tier0" => (
                    "tier0_high_latency".to_string(),
                    "p99",
                    thresholds::TIER0_P99_SECONDS,
                ),
                "params_tier1" => (
                    "params_tier1_high_latency".to_string(),
                    "p99",
                    thresholds::PARAMS_P99_SECONDS,
                ),
                "tier1_query" => (
                    "tier1_query_high_latency".to_string(),
                    "processing p99",
                    thresholds::TIER1_PROCESSING_P99_SECONDS,
                ),
                _ => unreachable!(),
            };
            let latency = window.alert_latency(endpoint);
            conditions.insert(
                latency_check,
                (
                    latency.samples >= thresholds::LATENCY_MIN_REQUESTS
                        && latency.p99.is_some_and(|p99| p99 > latency_threshold),
                    latency
                        .p99
                        .map(|p99| {
                            format!(
                                "{latency_label} {p99:.3}s over {:.0} samples",
                                latency.samples
                            )
                        })
                        .unwrap_or_else(|| format!("{latency_label} unavailable")),
                    format!(
                        "{latency_label} > {latency_threshold:.3}s over 5m, min {:.0}",
                        thresholds::LATENCY_MIN_REQUESTS
                    ),
                ),
            );
        }

        conditions.insert(
            "disk_usage".to_string(),
            (
                input.host.disk_used_ratio > thresholds::DISK_USED_RATIO,
                format!("{:.1}% used", input.host.disk_used_ratio * 100.0),
                format!("> {:.0}%", thresholds::DISK_USED_RATIO * 100.0),
            ),
        );
        conditions.insert(
            "memory_available".to_string(),
            (
                input.host.available_memory_bytes < thresholds::MEMORY_AVAILABLE_BYTES,
                format!(
                    "{} MiB available",
                    input.host.available_memory_bytes / 1024 / 1024
                ),
                format!("< {} MiB", thresholds::MEMORY_AVAILABLE_BYTES / 1024 / 1024),
            ),
        );

        let mut transitions = Vec::new();
        for (check, (firing, observed, threshold)) in conditions {
            match (firing, self.active.get(&check).cloned()) {
                (true, None) => {
                    let alert = Alert {
                        check: check.clone(),
                        observed,
                        threshold,
                        fired_at: SystemTime::now(),
                    };
                    self.active.insert(check, alert.clone());
                    self.record(format!("FIRED {}: {}", alert.check, alert.observed));
                    transitions.push(AlertTransition::Fired(alert));
                }
                (false, Some(alert)) => {
                    self.active.remove(&check);
                    self.record(format!("RECOVERED {}", alert.check));
                    transitions.push(AlertTransition::Recovered(alert));
                }
                _ => {}
            }
        }
        transitions
    }

    fn record(&mut self, message: String) {
        self.recent.push_front((SystemTime::now(), message));
        self.recent.truncate(20);
    }

    pub fn active(&self) -> Vec<Alert> {
        self.active.values().cloned().collect()
    }

    pub fn recent(&self) -> Vec<(SystemTime, String)> {
        self.recent.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::LatencyWindow;

    fn healthy_host() -> HostHealth {
        HostHealth {
            load_one: 0.1,
            load_five: 0.1,
            load_fifteen: 0.1,
            total_memory_bytes: 8 * 1024 * 1024 * 1024,
            available_memory_bytes: 4 * 1024 * 1024 * 1024,
            disk_total_bytes: 100,
            disk_available_bytes: 50,
            disk_used_ratio: 0.5,
            data_dir: "/data".into(),
        }
    }

    #[test]
    fn fires_once_per_episode_and_recovers_once() {
        let now = Instant::now();
        let host = healthy_host();
        let endpoints = BTreeMap::new();
        let mut engine = AlertEngine::default();
        assert!(engine
            .evaluate(AlertInput {
                now,
                scrape_ok: false,
                ready_ok: true,
                endpoints: &endpoints,
                host: &host,
            })
            .is_empty());
        assert!(matches!(
            engine
                .evaluate(AlertInput {
                    now: now + Duration::from_secs(15),
                    scrape_ok: false,
                    ready_ok: true,
                    endpoints: &endpoints,
                    host: &host,
                })
                .as_slice(),
            [AlertTransition::Fired(_)]
        ));
        assert!(engine
            .evaluate(AlertInput {
                now: now + Duration::from_secs(30),
                scrape_ok: false,
                ready_ok: true,
                endpoints: &endpoints,
                host: &host,
            })
            .is_empty());
        assert!(matches!(
            engine
                .evaluate(AlertInput {
                    now: now + Duration::from_secs(45),
                    scrape_ok: true,
                    ready_ok: true,
                    endpoints: &endpoints,
                    host: &host,
                })
                .as_slice(),
            [AlertTransition::Recovered(_)]
        ));
    }

    #[test]
    fn ready_requires_five_continuous_minutes() {
        let now = Instant::now();
        let host = healthy_host();
        let endpoints = BTreeMap::new();
        let mut engine = AlertEngine::default();
        engine.evaluate(AlertInput {
            now,
            scrape_ok: true,
            ready_ok: false,
            endpoints: &endpoints,
            host: &host,
        });
        assert!(engine
            .evaluate(AlertInput {
                now: now + Duration::from_secs(299),
                scrape_ok: true,
                ready_ok: false,
                endpoints: &endpoints,
                host: &host,
            })
            .is_empty());
        assert!(matches!(
            engine
                .evaluate(AlertInput {
                    now: now + Duration::from_secs(300),
                    scrape_ok: true,
                    ready_ok: false,
                    endpoints: &endpoints,
                    host: &host,
                })
                .as_slice(),
            [AlertTransition::Fired(_)]
        ));
    }

    #[test]
    fn applies_minimum_volume_to_thresholds() {
        let now = Instant::now();
        let host = healthy_host();
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "tier0".into(),
            EndpointWindow {
                requests: 9.0,
                errors_5xx: 9.0,
                error_ratio: 1.0,
                observed: LatencyWindow {
                    samples: 9.0,
                    p99: Some(9.0),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let mut engine = AlertEngine::default();
        assert!(engine
            .evaluate(AlertInput {
                now,
                scrape_ok: true,
                ready_ok: true,
                endpoints: &endpoints,
                host: &host,
            })
            .is_empty());
        endpoints.get_mut("tier0").unwrap().requests = 20.0;
        endpoints.get_mut("tier0").unwrap().observed.samples = 20.0;
        let fired = engine.evaluate(AlertInput {
            now: now + Duration::from_secs(15),
            scrape_ok: true,
            ready_ok: true,
            endpoints: &endpoints,
            host: &host,
        });
        assert_eq!(fired.len(), 2);
    }

    #[test]
    fn tier1_upload_latency_is_informational_but_processing_latency_pages() {
        let now = Instant::now();
        let host = healthy_host();
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "tier1_query".into(),
            EndpointWindow {
                requests: 20.0,
                observed: LatencyWindow {
                    samples: 20.0,
                    p99: Some(10.0),
                    ..Default::default()
                },
                processing: LatencyWindow {
                    samples: 20.0,
                    p99: Some(0.3),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let mut engine = AlertEngine::default();
        assert!(engine
            .evaluate(AlertInput {
                now,
                scrape_ok: true,
                ready_ok: true,
                endpoints: &endpoints,
                host: &host,
            })
            .is_empty());

        endpoints.get_mut("tier1_query").unwrap().requests = 100.0;
        endpoints.get_mut("tier1_query").unwrap().processing.samples = 19.0;
        endpoints.get_mut("tier1_query").unwrap().processing.p99 = Some(3.0);
        assert!(engine
            .evaluate(AlertInput {
                now: now + Duration::from_secs(15),
                scrape_ok: true,
                ready_ok: true,
                endpoints: &endpoints,
                host: &host,
            })
            .is_empty());

        endpoints.get_mut("tier1_query").unwrap().processing.samples = 20.0;
        let fired = engine.evaluate(AlertInput {
            now: now + Duration::from_secs(30),
            scrape_ok: true,
            ready_ok: true,
            endpoints: &endpoints,
            host: &host,
        });
        let [AlertTransition::Fired(alert)] = fired.as_slice() else {
            panic!("expected one processing-latency alert");
        };
        assert_eq!(alert.check, "tier1_query_high_latency");
        assert!(alert.observed.contains("processing p99"));
    }

    #[test]
    fn old_server_metrics_do_not_fall_back_to_observed_tier1_latency() {
        let now = Instant::now();
        let host = healthy_host();
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "tier1_query".into(),
            EndpointWindow {
                requests: 100.0,
                observed: LatencyWindow {
                    samples: 100.0,
                    p99: Some(10.0),
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        let mut engine = AlertEngine::default();
        assert!(engine
            .evaluate(AlertInput {
                now,
                scrape_ok: true,
                ready_ok: true,
                endpoints: &endpoints,
                host: &host,
            })
            .is_empty());
    }
}
