#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot<T> {
    pub value: T,
    pub source: String,
    pub sampled_at_unix_ms: i64,
}

impl<T> Snapshot<T> {
    pub fn new(value: T, source: impl Into<String>, sampled_at_unix_ms: i64) -> Self {
        Self {
            value,
            source: source.into(),
            sampled_at_unix_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability<T> {
    Available(T),
    Unsupported { reason: String },
}

impl<T> Capability<T> {
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self::Unsupported {
            reason: reason.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Capability, Snapshot};

    #[test]
    fn snapshot_carries_source_and_sample_time_with_the_value() {
        let snapshot = Snapshot::new(42_u8, "PDH", 1_725_000_000);

        assert_eq!(snapshot.value, 42);
        assert_eq!(snapshot.source, "PDH");
        assert_eq!(snapshot.sampled_at_unix_ms, 1_725_000_000);
    }

    #[test]
    fn unsupported_capability_preserves_the_reason_for_the_ui() {
        let capability: Capability<f32> = Capability::unsupported("Sensor não exposto pelo driver");

        assert_eq!(
            capability,
            Capability::Unsupported {
                reason: "Sensor não exposto pelo driver".to_owned(),
            }
        );
    }
}
