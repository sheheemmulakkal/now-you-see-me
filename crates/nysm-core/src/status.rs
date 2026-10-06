use serde::{Deserialize, Serialize};

/// Why a value is (or is not) present. Missing values are never zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Available,
    /// A rate needs a second counter sample; not an error.
    WarmingUp,
    /// This platform/kernel/hardware does not expose the metric.
    Unsupported,
    PermissionDenied,
    /// The last good value is older than its freshness bound.
    Stale,
    CollectionError,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Available => "available",
            Status::WarmingUp => "warming up",
            Status::Unsupported => "unsupported",
            Status::PermissionDenied => "permission denied",
            Status::Stale => "stale",
            Status::CollectionError => "collection error",
        }
    }
}

/// A value together with its status. `value` is present only when the
/// status is `Available` or `Stale` (stale values are labelled, never live).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reading<T> {
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl<T> Reading<T> {
    pub fn ok(value: T) -> Self {
        Reading {
            status: Status::Available,
            value: Some(value),
            reason: None,
        }
    }

    pub fn missing(status: Status, reason: impl Into<String>) -> Self {
        debug_assert!(status != Status::Available);
        Reading {
            status,
            value: None,
            reason: Some(reason.into()),
        }
    }

    pub fn warming_up() -> Self {
        Reading::missing(Status::WarmingUp, "waiting for a second counter sample")
    }

    pub fn stale(value: T, reason: impl Into<String>) -> Self {
        Reading {
            status: Status::Stale,
            value: Some(value),
            reason: Some(reason.into()),
        }
    }

    /// The value only if it is live.
    pub fn live(&self) -> Option<&T> {
        match self.status {
            Status::Available => self.value.as_ref(),
            _ => None,
        }
    }

    pub fn is_available(&self) -> bool {
        self.status == Status::Available
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Reading<U> {
        Reading {
            status: self.status,
            value: self.value.map(f),
            reason: self.reason,
        }
    }

    pub fn as_ref(&self) -> Reading<&T> {
        Reading {
            status: self.status,
            value: self.value.as_ref(),
            reason: self.reason.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_values_serialize_without_value() {
        let r: Reading<f64> = Reading::warming_up();
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"warming_up\""));
        assert!(!json.contains("\"value\""));
        assert!(r.live().is_none());
    }

    #[test]
    fn stale_values_are_not_live() {
        let r = Reading::stale(5u64, "old");
        assert_eq!(r.value, Some(5));
        assert!(r.live().is_none());
    }
}
