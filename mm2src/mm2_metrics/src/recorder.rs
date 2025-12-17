use crate::{mm_metrics::MmHistogram, MetricType, MetricsJson};

use metrics::{Counter, Gauge, Histogram, Key, KeyName, Label, Recorder, Unit};
#[cfg(not(target_arch = "wasm32"))]
use metrics_exporter_prometheus::formatting::{key_to_parts, write_metric_line, write_type_line};
use metrics_util::registry::{GenerationalAtomicStorage, GenerationalStorage, Registry};
use std::sync::{atomic::Ordering, Arc};
use std::{collections::HashMap, slice::Iter};

pub struct Snapshot {
    pub counters: HashMap<String, HashMap<Vec<String>, u64>>,
    pub gauges: HashMap<String, HashMap<Vec<String>, f64>>,
    pub histograms: HashMap<String, HashMap<Vec<String>, Vec<f64>>>,
}

/// `MmRecorder` the core of mm metrics.
///
///  Registering, Recording, Updating and Collecting metrics is all done from within MmRecorder.
pub struct MmRecorder {
    pub(crate) registry: Registry<Key, GenerationalAtomicStorage>,
}

impl Default for MmRecorder {
    fn default() -> Self {
        Self {
            registry: Registry::new(GenerationalStorage::atomic()),
        }
    }
}

impl MmRecorder {
    #[cfg(not(target_arch = "wasm32"))]
    fn get_metrics(&self) -> Snapshot {
        let mut counters = HashMap::new();
        for (key, counter) in self.registry.get_counter_handles() {
            key_value_to_snapshot_entry(&mut counters, key, counter.get_inner().load(Ordering::Acquire));
        }

        let mut gauges = HashMap::new();
        for (key, gauge) in self.registry.get_gauge_handles() {
            key_value_to_snapshot_entry(
                &mut gauges,
                key,
                f64::from_bits(gauge.get_inner().load(Ordering::Acquire)),
            );
        }

        let mut histograms = HashMap::new();
        for (key, histogram) in self.registry.get_histogram_handles() {
            key_value_to_snapshot_entry(&mut histograms, key, histogram.get_inner().data());
        }

        Snapshot {
            counters,
            gauges,
            histograms,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_prometheus(&self) -> String {
        let Snapshot {
            mut counters,
            mut histograms,
            mut gauges,
        } = self.get_metrics();

        let mut output = String::new();

        for (name, mut by_labels) in counters.drain() {
            write_type_line(&mut output, name.as_str(), "counter");
            for (labels, value) in by_labels.drain() {
                write_metric_line::<&str, u64>(&mut output, &name, None, &labels, None, value);
            }
            output.push('\n');
        }

        for (name, mut by_labels) in gauges.drain() {
            write_type_line(&mut output, name.as_str(), "gauge");
            for (labels, value) in by_labels.drain() {
                write_metric_line::<&str, f64>(&mut output, &name, None, &labels, None, value);
            }
            output.push('\n');
        }

        for (key, histogram) in histograms.drain() {
            let key = Key::from_name(key.to_owned());
            let (name, labels) = key_to_parts(&key, None);
            write_type_line(&mut output, &name, "histogram");
            for (_, values) in histogram {
                let mut count = 0;
                let mut sum = 0.0;

                count += values.len();
                values.iter().for_each(|value| {
                    sum += value;
                });

                write_metric_line::<&str, f64>(&mut output, &name, Some("sum"), &labels, None, sum);
                write_metric_line::<&str, usize>(
                    &mut output,
                    &name,
                    Some("count"),
                    key_to_parts(&key, None).1.as_slice(),
                    None,
                    count,
                );
                output.push('\n');
            }
        }

        output
    }

    pub fn prepare_json(&self) -> MetricsJson {
        let mut output = vec![];

        for (key, counter) in self.registry.get_counter_handles() {
            let (key, labels) = key.into_parts();
            let value = counter.get_inner().load(Ordering::Acquire);
            output.push(MetricType::Counter {
                key: key.as_str().to_string(),
                labels: labels_into_parts(labels.clone().iter()),
                value,
            });
        }

        for (key, gauge) in self.registry.get_gauge_handles() {
            let (key, labels) = key.into_parts();
            let value = f64::from_bits(gauge.get_inner().load(Ordering::Acquire));
            output.push(MetricType::Gauge {
                key: key.as_str().to_string(),
                labels: labels_into_parts(labels.clone().iter()),
                value,
            });
        }

        for (key, histogram) in self.registry.get_histogram_handles() {
            let value = histogram.get_inner().data();
            let (key, labels) = key.into_parts();
            let mm_histogram = MmHistogram::new(&value);

            if let Some(qauntiles_value) = mm_histogram {
                output.push(MetricType::Histogram {
                    key: key.as_str().to_string(),
                    labels: labels_into_parts(labels.clone().iter()),
                    quantiles: qauntiles_value.to_json_quantiles(),
                });
            }
        }

        MetricsJson { metrics: output }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn key_value_to_snapshot_entry<V>(metrics: &mut HashMap<String, HashMap<Vec<String>, V>>, key: Key, value: V) {
    let (key_name, labels) = key_to_parts(&key, None);
    let metrics = metrics.entry(key_name).or_insert_with(|| HashMap::new());
    metrics.insert(labels, value);
}

impl Recorder for MmRecorder {
    fn describe_counter(&self, _key: KeyName, _unit: Option<Unit>, _description: metrics::SharedString) {
        // mm2_metrics doesn't use this method
    }

    fn describe_gauge(&self, _key: KeyName, _unit: Option<Unit>, _description: metrics::SharedString) {
        // mm2_metrics doesn't use this method
    }

    fn describe_histogram(&self, _key: KeyName, _unit: Option<Unit>, _description: metrics::SharedString) {
        // mm2_metrics doesn't use this method
    }

    fn register_counter(&self, key: &Key) -> Counter {
        self.registry.get_or_create_counter(key, |e| e.clone().into())
    }

    fn register_gauge(&self, key: &Key) -> Gauge {
        self.registry.get_or_create_gauge(key, |e| e.clone().into())
    }

    fn register_histogram(&self, key: &Key) -> Histogram {
        self.registry.get_or_create_histogram(key, |e| e.clone().into())
    }
}

pub trait TryRecorder {
    /// Check for recorder and set one if none is set.
    fn try_recorder(&self) -> Option<Arc<MmRecorder>>;
}

/// Used for parsing `Iter<Label>` into `Key` and `Value`.
fn labels_into_parts(labels: Iter<Label>) -> HashMap<String, String> {
    labels
        .map(|label| (label.key().to_string(), label.value().to_string()))
        .collect()
}
