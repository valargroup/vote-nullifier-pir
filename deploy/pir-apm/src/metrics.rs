use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    time::Instant,
};

pub const ENDPOINTS: [&str; 3] = ["tier0", "params_tier1", "tier1_query"];
const MAX_SNAPSHOTS: usize = 21;

#[derive(Clone, Debug, Default)]
pub struct HistogramCumulative {
    pub buckets: Vec<(f64, f64)>,
    pub sum: f64,
    pub count: f64,
}

#[derive(Clone, Debug, Default)]
pub struct EndpointCumulative {
    pub requests: f64,
    pub errors_5xx: f64,
    pub observed: HistogramCumulative,
    pub processing: HistogramCumulative,
    pub in_flight: f64,
    pub processing_in_flight: f64,
}

#[derive(Clone, Debug)]
pub struct MetricsSnapshot {
    pub at: Instant,
    pub endpoints: BTreeMap<String, EndpointCumulative>,
    pub snapshot_gauges: BTreeMap<String, f64>,
    pub resident_memory_bytes: Option<f64>,
    pub process_start_time_seconds: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct LatencyWindow {
    pub samples: f64,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct EndpointWindow {
    pub qps: f64,
    pub requests: f64,
    pub errors_5xx: f64,
    pub error_ratio: f64,
    pub observed: LatencyWindow,
    pub processing: LatencyWindow,
    pub in_flight: f64,
    pub processing_in_flight: f64,
}

impl EndpointWindow {
    /// Latency distribution used for paging this endpoint.
    pub fn alert_latency(&self, endpoint: &str) -> &LatencyWindow {
        if endpoint == "tier1_query" {
            &self.processing
        } else {
            &self.observed
        }
    }
}

#[derive(Default)]
pub struct RollingMetrics {
    snapshots: VecDeque<MetricsSnapshot>,
}

impl RollingMetrics {
    pub fn push(&mut self, snapshot: MetricsSnapshot) {
        self.snapshots.push_back(snapshot);
        while self.snapshots.len() > MAX_SNAPSHOTS {
            self.snapshots.pop_front();
        }
    }

    pub fn latest(&self) -> Option<&MetricsSnapshot> {
        self.snapshots.back()
    }

    pub fn windows(&self) -> BTreeMap<String, EndpointWindow> {
        let Some(newest) = self.snapshots.back() else {
            return BTreeMap::new();
        };
        let oldest_index = self
            .snapshots
            .iter()
            .position(|snapshot| newest.at.duration_since(snapshot.at).as_secs() <= 300)
            .unwrap_or(self.snapshots.len() - 1);
        let oldest = &self.snapshots[oldest_index];
        let elapsed = newest.at.duration_since(oldest.at).as_secs_f64().max(1.0);

        ENDPOINTS
            .iter()
            .map(|endpoint| {
                let current = newest.endpoints.get(*endpoint).cloned().unwrap_or_default();
                let mut requests = 0.0;
                let mut errors = 0.0;
                for index in (oldest_index + 1)..self.snapshots.len() {
                    let previous_snapshot = &self.snapshots[index - 1];
                    let next_snapshot = &self.snapshots[index];
                    let reset = process_generation_changed(previous_snapshot, next_snapshot);
                    let previous = previous_snapshot
                        .endpoints
                        .get(*endpoint)
                        .cloned()
                        .unwrap_or_default();
                    let next = next_snapshot
                        .endpoints
                        .get(*endpoint)
                        .cloned()
                        .unwrap_or_default();
                    requests += counter_delta(next.requests, previous.requests, reset);
                    errors += counter_delta(next.errors_5xx, previous.errors_5xx, reset);
                }
                (
                    (*endpoint).to_string(),
                    EndpointWindow {
                        qps: requests / elapsed,
                        requests,
                        errors_5xx: errors,
                        error_ratio: if requests > 0.0 {
                            errors / requests
                        } else {
                            0.0
                        },
                        observed: latency_window(
                            &self.snapshots,
                            oldest_index,
                            endpoint,
                            observed_histogram,
                        ),
                        processing: latency_window(
                            &self.snapshots,
                            oldest_index,
                            endpoint,
                            processing_histogram,
                        ),
                        in_flight: current.in_flight,
                        processing_in_flight: current.processing_in_flight,
                    },
                )
            })
            .collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.snapshots.len()
    }
}

fn observed_histogram(values: &EndpointCumulative) -> &HistogramCumulative {
    &values.observed
}

fn processing_histogram(values: &EndpointCumulative) -> &HistogramCumulative {
    &values.processing
}

fn latency_window(
    snapshots: &VecDeque<MetricsSnapshot>,
    oldest_index: usize,
    endpoint: &str,
    histogram: fn(&EndpointCumulative) -> &HistogramCumulative,
) -> LatencyWindow {
    let current_endpoint = snapshots
        .back()
        .and_then(|snapshot| snapshot.endpoints.get(endpoint))
        .cloned()
        .unwrap_or_default();
    let mut buckets: Vec<(f64, f64)> = histogram(&current_endpoint)
        .buckets
        .iter()
        .map(|(upper, _)| (*upper, 0.0))
        .collect();
    let mut samples = 0.0;
    for index in (oldest_index + 1)..snapshots.len() {
        let previous_snapshot = &snapshots[index - 1];
        let next_snapshot = &snapshots[index];
        let generation_changed = process_generation_changed(previous_snapshot, next_snapshot);
        let previous = previous_snapshot
            .endpoints
            .get(endpoint)
            .cloned()
            .unwrap_or_default();
        let next = next_snapshot
            .endpoints
            .get(endpoint)
            .cloned()
            .unwrap_or_default();
        let previous = histogram(&previous);
        let next = histogram(&next);
        let reset = generation_changed || next.count < previous.count;
        samples += if reset {
            next.count
        } else {
            next.count - previous.count
        };
        // A histogram resets as one metric family. Differencing its buckets
        // independently across a process restart can make them non-monotonic.
        let delta = if reset {
            next.buckets.clone()
        } else {
            histogram_delta(&next.buckets, &previous.buckets)
        };
        for (upper, total) in &mut buckets {
            *total += delta
                .iter()
                .find(|(delta_upper, _)| delta_upper == upper)
                .map(|(_, value)| *value)
                .unwrap_or(0.0);
        }
    }
    LatencyWindow {
        samples,
        p50: histogram_quantile(0.50, &buckets, samples),
        p95: histogram_quantile(0.95, &buckets, samples),
        p99: histogram_quantile(0.99, &buckets, samples),
    }
}

fn process_generation_changed(previous: &MetricsSnapshot, next: &MetricsSnapshot) -> bool {
    matches!(
        (
            previous.process_start_time_seconds,
            next.process_start_time_seconds,
        ),
        (Some(previous), Some(next)) if previous != next
    )
}

fn counter_delta(current: f64, previous: f64, reset: bool) -> f64 {
    if reset || current < previous {
        current
    } else {
        current - previous
    }
}

pub fn histogram_delta(current: &[(f64, f64)], previous: &[(f64, f64)]) -> Vec<(f64, f64)> {
    current
        .iter()
        .map(|(upper, value)| {
            let old = previous
                .iter()
                .find(|(old_upper, _)| old_upper == upper)
                .map(|(_, value)| *value)
                .unwrap_or(0.0);
            (*upper, counter_delta(*value, old, false))
        })
        .collect()
}

pub fn histogram_quantile(q: f64, cumulative_buckets: &[(f64, f64)], count: f64) -> Option<f64> {
    if !(0.0..=1.0).contains(&q) || count <= 0.0 || cumulative_buckets.is_empty() {
        return None;
    }
    let rank = q * count;
    let mut previous_count = 0.0;
    let mut previous_upper = 0.0;
    for (upper, bucket_count) in cumulative_buckets {
        if *bucket_count >= rank {
            if upper.is_infinite() {
                return Some(previous_upper);
            }
            let observations = (*bucket_count - previous_count).max(0.0);
            if observations == 0.0 {
                return Some(*upper);
            }
            return Some(
                previous_upper + (*upper - previous_upper) * (rank - previous_count) / observations,
            );
        }
        previous_count = *bucket_count;
        if upper.is_finite() {
            previous_upper = *upper;
        }
    }
    Some(previous_upper)
}

#[derive(Debug)]
struct ParsedSample {
    name: String,
    labels: HashMap<String, String>,
    value: f64,
}

pub fn parse_prometheus(text: &str, at: Instant) -> Result<MetricsSnapshot, String> {
    let mut endpoints: BTreeMap<String, EndpointCumulative> = ENDPOINTS
        .iter()
        .map(|name| ((*name).to_string(), EndpointCumulative::default()))
        .collect();
    let mut snapshot_gauges = BTreeMap::new();
    let mut resident_memory_bytes = None;
    let mut process_start_time_seconds = None;

    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let sample = parse_line(line)
            .map_err(|error| format!("line {}: {error}", line_number.saturating_add(1)))?;
        match sample.name.as_str() {
            "nf_http_requests_total" => {
                if let Some(endpoint) = sample.labels.get("endpoint") {
                    if let Some(values) = endpoints.get_mut(endpoint) {
                        values.requests += sample.value;
                        if sample
                            .labels
                            .get("status")
                            .is_some_and(|status| status.starts_with('5'))
                        {
                            values.errors_5xx += sample.value;
                        }
                    }
                }
            }
            "nf_http_request_duration_seconds_bucket" => {
                set_histogram_bucket(&mut endpoints, &sample, |values| &mut values.observed)?
            }
            "nf_http_request_duration_seconds_sum" => set_histogram_value(
                &mut endpoints,
                &sample,
                |values| &mut values.observed,
                |h| h.sum = sample.value,
            ),
            "nf_http_request_duration_seconds_count" => set_histogram_value(
                &mut endpoints,
                &sample,
                |values| &mut values.observed,
                |h| h.count = sample.value,
            ),
            "nf_http_request_processing_duration_seconds_bucket" => {
                set_histogram_bucket(&mut endpoints, &sample, |values| &mut values.processing)?
            }
            "nf_http_request_processing_duration_seconds_sum" => set_histogram_value(
                &mut endpoints,
                &sample,
                |values| &mut values.processing,
                |histogram| histogram.sum = sample.value,
            ),
            "nf_http_request_processing_duration_seconds_count" => set_histogram_value(
                &mut endpoints,
                &sample,
                |values| &mut values.processing,
                |histogram| histogram.count = sample.value,
            ),
            "nf_http_in_flight" => set_endpoint_value(&mut endpoints, &sample, |values| {
                values.in_flight = sample.value
            }),
            "nf_http_processing_in_flight" => {
                set_endpoint_value(&mut endpoints, &sample, |values| {
                    values.processing_in_flight = sample.value
                })
            }
            "process_resident_memory_bytes" => resident_memory_bytes = Some(sample.value),
            "process_start_time_seconds" => process_start_time_seconds = Some(sample.value),
            name if name.starts_with("nf_snapshot_") => {
                snapshot_gauges.insert(name.to_string(), sample.value);
            }
            _ => {}
        }
    }
    for endpoint in endpoints.values_mut() {
        endpoint
            .observed
            .buckets
            .sort_by(|(left, _), (right, _)| left.total_cmp(right));
        endpoint
            .processing
            .buckets
            .sort_by(|(left, _), (right, _)| left.total_cmp(right));
    }
    Ok(MetricsSnapshot {
        at,
        endpoints,
        snapshot_gauges,
        resident_memory_bytes,
        process_start_time_seconds,
    })
}

fn set_histogram_bucket(
    endpoints: &mut BTreeMap<String, EndpointCumulative>,
    sample: &ParsedSample,
    histogram: fn(&mut EndpointCumulative) -> &mut HistogramCumulative,
) -> Result<(), String> {
    let (Some(endpoint), Some(le)) = (sample.labels.get("endpoint"), sample.labels.get("le"))
    else {
        return Ok(());
    };
    let Some(values) = endpoints.get_mut(endpoint) else {
        return Ok(());
    };
    let upper = if le == "+Inf" {
        f64::INFINITY
    } else {
        le.parse::<f64>()
            .map_err(|_| format!("invalid histogram bound {le:?}"))?
    };
    histogram(values).buckets.push((upper, sample.value));
    Ok(())
}

fn set_histogram_value(
    endpoints: &mut BTreeMap<String, EndpointCumulative>,
    sample: &ParsedSample,
    histogram: fn(&mut EndpointCumulative) -> &mut HistogramCumulative,
    set: impl FnOnce(&mut HistogramCumulative),
) {
    if let Some(values) = sample
        .labels
        .get("endpoint")
        .and_then(|endpoint| endpoints.get_mut(endpoint))
    {
        set(histogram(values));
    }
}

fn set_endpoint_value(
    endpoints: &mut BTreeMap<String, EndpointCumulative>,
    sample: &ParsedSample,
    set: impl FnOnce(&mut EndpointCumulative),
) {
    if let Some(values) = sample
        .labels
        .get("endpoint")
        .and_then(|endpoint| endpoints.get_mut(endpoint))
    {
        set(values);
    }
}

fn parse_line(line: &str) -> Result<ParsedSample, String> {
    let split = line
        .rfind(char::is_whitespace)
        .ok_or_else(|| "missing metric value".to_string())?;
    let descriptor = line[..split].trim_end();
    let value_text = line[split..].trim();
    let value = value_text
        .parse::<f64>()
        .map_err(|_| format!("invalid metric value {value_text:?}"))?;

    let (name, labels) = if let Some(open) = descriptor.find('{') {
        if !descriptor.ends_with('}') {
            return Err("unterminated label set".to_string());
        }
        (
            descriptor[..open].to_string(),
            parse_labels(&descriptor[open + 1..descriptor.len() - 1])?,
        )
    } else {
        (descriptor.to_string(), HashMap::new())
    };
    if name.is_empty() {
        return Err("empty metric name".to_string());
    }
    Ok(ParsedSample {
        name,
        labels,
        value,
    })
}

fn parse_labels(input: &str) -> Result<HashMap<String, String>, String> {
    let mut labels = HashMap::new();
    let bytes = input.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        while cursor < bytes.len() && (bytes[cursor] == b',' || bytes[cursor].is_ascii_whitespace())
        {
            cursor += 1;
        }
        if cursor == bytes.len() {
            break;
        }
        let key_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != b'=' {
            cursor += 1;
        }
        if cursor == bytes.len() {
            return Err("label missing '='".to_string());
        }
        let key = input[key_start..cursor].trim().to_string();
        cursor += 1;
        if cursor >= bytes.len() || bytes[cursor] != b'"' {
            return Err("label value must be quoted".to_string());
        }
        cursor += 1;
        let mut value = String::new();
        let mut closed = false;
        while cursor < bytes.len() {
            match bytes[cursor] {
                b'"' => {
                    cursor += 1;
                    closed = true;
                    break;
                }
                b'\\' => {
                    cursor += 1;
                    if cursor >= bytes.len() {
                        return Err("unterminated label escape".to_string());
                    }
                    value.push(match bytes[cursor] {
                        b'n' => '\n',
                        b'\\' => '\\',
                        b'"' => '"',
                        other => other as char,
                    });
                    cursor += 1;
                }
                byte => {
                    value.push(byte as char);
                    cursor += 1;
                }
            }
        }
        if !closed {
            return Err("unterminated quoted label value".to_string());
        }
        labels.insert(key, value);
    }
    Ok(labels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn parses_required_prometheus_families_and_labels() {
        let text = r#"
# HELP ignored comment
nf_http_requests_total{endpoint="tier0",method="GET",status="200"} 90
nf_http_requests_total{endpoint="tier0",method="GET",status="503"} 10
nf_http_request_duration_seconds_bucket{endpoint="tier0",le="0.5"} 60
nf_http_request_duration_seconds_bucket{endpoint="tier0",le="1"} 95
nf_http_request_duration_seconds_bucket{endpoint="tier0",le="+Inf"} 100
nf_http_request_duration_seconds_sum{endpoint="tier0"} 40
nf_http_request_duration_seconds_count{endpoint="tier0"} 100
nf_http_in_flight{endpoint="tier0"} 3
nf_http_request_processing_duration_seconds_bucket{endpoint="tier1_query",le="0.5"} 9
nf_http_request_processing_duration_seconds_bucket{endpoint="tier1_query",le="+Inf"} 10
nf_http_request_processing_duration_seconds_sum{endpoint="tier1_query"} 2
nf_http_request_processing_duration_seconds_count{endpoint="tier1_query"} 10
nf_http_processing_in_flight{endpoint="tier1_query"} 2
nf_snapshot_served_height 123
nf_snapshot_expected_height 124
process_resident_memory_bytes 1048576
process_start_time_seconds 1787880000
"#;
        let parsed = parse_prometheus(text, Instant::now()).unwrap();
        let tier0 = &parsed.endpoints["tier0"];
        assert_eq!(tier0.requests, 100.0);
        assert_eq!(tier0.errors_5xx, 10.0);
        assert_eq!(tier0.in_flight, 3.0);
        assert_eq!(tier0.observed.sum, 40.0);
        assert_eq!(tier0.observed.count, 100.0);
        assert_eq!(tier0.observed.buckets.len(), 3);
        let tier1 = &parsed.endpoints["tier1_query"];
        assert_eq!(tier1.processing.sum, 2.0);
        assert_eq!(tier1.processing.count, 10.0);
        assert_eq!(tier1.processing_in_flight, 2.0);
        assert_eq!(parsed.snapshot_gauges["nf_snapshot_served_height"], 123.0);
        assert_eq!(parsed.resident_memory_bytes, Some(1_048_576.0));
        assert_eq!(parsed.process_start_time_seconds, Some(1_787_880_000.0));
    }

    #[test]
    fn parses_escaped_label_values() {
        let sample = parse_line(r#"metric{a="quote\"",b="slash\\",c="line\n"} 7"#).unwrap();
        assert_eq!(sample.labels["a"], "quote\"");
        assert_eq!(sample.labels["b"], "slash\\");
        assert_eq!(sample.labels["c"], "line\n");
        assert!(parse_line(r#"metric{a="unterminated} 7"#).is_err());
    }

    #[test]
    fn computes_histogram_deltas_and_interpolated_quantiles() {
        let old = vec![(0.5, 10.0), (1.0, 20.0), (f64::INFINITY, 20.0)];
        let new = vec![(0.5, 20.0), (1.0, 40.0), (f64::INFINITY, 40.0)];
        let delta = histogram_delta(&new, &old);
        assert_eq!(delta[0].1, 10.0);
        assert!((histogram_quantile(0.5, &delta, 20.0).unwrap() - 0.5).abs() < 1e-9);
        assert!((histogram_quantile(0.75, &delta, 20.0).unwrap() - 0.75).abs() < 1e-9);
        assert_eq!(
            histogram_quantile(0.99, &[(1.0, 5.0), (f64::INFINITY, 10.0)], 10.0),
            Some(1.0)
        );
    }

    #[test]
    fn keeps_observed_and_processing_latency_separate() {
        let start = Instant::now();
        let histogram = |count: f64, slow_upper: f64| HistogramCumulative {
            buckets: vec![
                (slow_upper / 2.0, 0.0),
                (slow_upper, count),
                (f64::INFINITY, count),
            ],
            sum: count * slow_upper,
            count,
        };
        let mut rolling = RollingMetrics::default();
        for (index, count) in [0.0, 20.0].into_iter().enumerate() {
            let mut endpoints = BTreeMap::new();
            endpoints.insert(
                "tier1_query".to_string(),
                EndpointCumulative {
                    requests: count,
                    observed: histogram(count, 10.0),
                    processing: histogram(count, 0.5),
                    processing_in_flight: index as f64,
                    ..Default::default()
                },
            );
            rolling.push(MetricsSnapshot {
                at: start + Duration::from_secs(index as u64 * 15),
                endpoints,
                snapshot_gauges: BTreeMap::new(),
                resident_memory_bytes: None,
                process_start_time_seconds: None,
            });
        }

        let window = &rolling.windows()["tier1_query"];
        assert_eq!(window.observed.samples, 20.0);
        assert_eq!(window.processing.samples, 20.0);
        assert!(window.observed.p99.unwrap() > 9.0);
        assert!(window.processing.p99.unwrap() < 0.5);
        assert_eq!(
            window.alert_latency("tier1_query").p99,
            window.processing.p99
        );
        assert_eq!(window.processing_in_flight, 1.0);
    }

    #[test]
    fn histogram_reset_uses_only_the_new_process_distribution() {
        fn before_restart(upper: f64) -> HistogramCumulative {
            HistogramCumulative {
                buckets: vec![
                    (upper, 15.0),
                    (upper * 2.0, 18.0),
                    (upper * 4.0, 20.0),
                    (f64::INFINITY, 100.0),
                ],
                sum: 500.0,
                count: 100.0,
            }
        }

        fn after_restart(upper: f64) -> HistogramCumulative {
            HistogramCumulative {
                buckets: vec![
                    (upper, 20.0),
                    (upper * 2.0, 20.0),
                    (upper * 4.0, 20.0),
                    (f64::INFINITY, 20.0),
                ],
                sum: upper * 10.0,
                count: 20.0,
            }
        }

        let start = Instant::now();
        let mut rolling = RollingMetrics::default();
        for index in 0..2 {
            let mut endpoints = BTreeMap::new();
            endpoints.insert(
                "tier1_query".to_string(),
                EndpointCumulative {
                    requests: if index == 0 { 100.0 } else { 20.0 },
                    observed: if index == 0 {
                        before_restart(10.0)
                    } else {
                        after_restart(10.0)
                    },
                    processing: if index == 0 {
                        before_restart(0.5)
                    } else {
                        after_restart(0.5)
                    },
                    ..Default::default()
                },
            );
            rolling.push(MetricsSnapshot {
                at: start + Duration::from_secs(index as u64 * 15),
                endpoints,
                snapshot_gauges: BTreeMap::new(),
                resident_memory_bytes: None,
                process_start_time_seconds: None,
            });
        }

        let window = &rolling.windows()["tier1_query"];
        assert_eq!(window.requests, 20.0);
        assert_eq!(window.observed.samples, 20.0);
        assert_eq!(window.processing.samples, 20.0);
        assert!(window.observed.p99.unwrap() < 10.0);
        assert!(window.processing.p99.unwrap() < 0.5);
    }

    #[test]
    fn process_start_time_detects_restart_when_counts_increase() {
        fn histogram(count: f64, fast: f64) -> HistogramCumulative {
            HistogramCumulative {
                buckets: vec![
                    (0.5, fast),
                    (2.0, fast),
                    (5.0, count),
                    (f64::INFINITY, count),
                ],
                sum: count * 3.0,
                count,
            }
        }

        let start = Instant::now();
        let mut rolling = RollingMetrics::default();
        for (index, count) in [10.0, 25.0].into_iter().enumerate() {
            let mut endpoints = BTreeMap::new();
            endpoints.insert(
                "tier1_query".to_string(),
                EndpointCumulative {
                    requests: count,
                    errors_5xx: if index == 0 { 1.0 } else { 3.0 },
                    processing: histogram(count, if index == 0 { count } else { 0.0 }),
                    ..Default::default()
                },
            );
            rolling.push(MetricsSnapshot {
                at: start + Duration::from_secs(index as u64 * 15),
                endpoints,
                snapshot_gauges: BTreeMap::new(),
                resident_memory_bytes: None,
                process_start_time_seconds: Some(100.0 + index as f64),
            });
        }

        let window = &rolling.windows()["tier1_query"];
        assert_eq!(window.requests, 25.0);
        assert_eq!(window.errors_5xx, 3.0);
        assert_eq!(window.processing.samples, 25.0);
        assert!(window.processing.p99.unwrap() > 2.0);
    }

    #[test]
    fn old_server_metrics_leave_tier1_processing_unavailable() {
        let parsed = parse_prometheus(
            r#"
nf_http_requests_total{endpoint="tier1_query",method="POST",status="200"} 20
nf_http_request_duration_seconds_bucket{endpoint="tier1_query",le="10"} 20
nf_http_request_duration_seconds_bucket{endpoint="tier1_query",le="+Inf"} 20
nf_http_request_duration_seconds_sum{endpoint="tier1_query"} 180
nf_http_request_duration_seconds_count{endpoint="tier1_query"} 20
"#,
            Instant::now(),
        )
        .unwrap();

        let tier1 = &parsed.endpoints["tier1_query"];
        assert_eq!(tier1.observed.count, 20.0);
        assert_eq!(tier1.processing.count, 0.0);
    }

    #[test]
    fn handles_counter_reset_and_caps_history() {
        let start = Instant::now();
        let mut rolling = RollingMetrics::default();
        for index in 0..25 {
            let mut endpoints = BTreeMap::new();
            endpoints.insert(
                "tier0".to_string(),
                EndpointCumulative {
                    requests: if index == 24 { 5.0 } else { index as f64 },
                    ..Default::default()
                },
            );
            rolling.push(MetricsSnapshot {
                at: start + Duration::from_secs(index * 15),
                endpoints,
                snapshot_gauges: BTreeMap::new(),
                resident_memory_bytes: None,
                process_start_time_seconds: None,
            });
        }
        assert_eq!(rolling.len(), 21);
        assert_eq!(rolling.windows()["tier0"].requests, 24.0);
    }
}
