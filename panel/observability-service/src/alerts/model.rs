//! What alert rules say, apart from how they are kept, read or sent.

use panel_domain::{RouteId, SiteId, UpstreamPoolId};
use panel_errors::{PanelError, Result};
use std::time::Duration;

/// The longest a condition may have to hold before its rule fires.
pub const MAX_PENDING: Duration = Duration::from_secs(86_400);
const MAX_ID: usize = 64;
const MAX_NAME: usize = 128;
const MAX_DESCRIPTION: usize = 1024;
const MAX_CHANNELS: usize = 16;

macro_rules! named {
    ($(#[$meta:meta])* $name:ident { $($(#[$variant_meta:meta])* $variant:ident => $text:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        #[non_exhaustive]
        pub enum $name {
            $($(#[$variant_meta])* $variant,)*
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];

            pub fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)*
                }
            }

            pub fn parse(text: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|value| value.name() == text)
            }
        }
    };
}

named!(
    /// What a rule reads, over the last five minutes.
    Measure {
        ServerErrorRatio => "server_error_ratio",
        LatencyP95 => "latency_p95",
        RequestRate => "request_rate",
        UpstreamErrorRatio => "upstream_error_ratio",
        OpenConnections => "open_connections",
    }
);

named!(Comparison {
    Above => "above",
    Below => "below",
});

named!(Severity {
    Warning => "warning",
    Critical => "critical",
});

named!(
    /// Where a rule stands.
    State {
        Inactive => "inactive",
        /// The condition holds, for less than the pending period so far.
        Pending => "pending",
        Firing => "firing",
    }
);

impl Measure {
    /// Whether it reads the gateway's requests, which sites and routes narrow.
    pub fn reads_requests(self) -> bool {
        matches!(
            self,
            Self::ServerErrorRatio | Self::LatencyP95 | Self::RequestRate
        )
    }

    pub fn is_ratio(self) -> bool {
        matches!(self, Self::ServerErrorRatio | Self::UpstreamErrorRatio)
    }
}

impl Comparison {
    pub fn holds(self, value: f64, threshold: f64) -> bool {
        match self {
            Self::Above => value > threshold,
            Self::Below => value < threshold,
        }
    }
}

/// A rule or channel ID: what site and route IDs allow, at most 64 bytes.
pub fn identifier(value: &str, what: &str) -> Result<String> {
    if value.len() > MAX_ID {
        return Err(PanelError::invalid_argument(format!(
            "{what} is longer than {MAX_ID} bytes"
        )));
    }
    SiteId::new(value)
        .map(|_| value.to_owned())
        .map_err(|error| PanelError::invalid_argument(format!("{what}: {error}")))
}

/// What a rule watches and whom it tells.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleSpec {
    pub name: String,
    pub description: String,
    pub measure: Measure,
    pub comparison: Comparison,
    pub threshold: f64,
    pub pending_for: Duration,
    pub site: Option<SiteId>,
    pub route: Option<RouteId>,
    pub upstream: Option<UpstreamPoolId>,
    pub severity: Severity,
    pub enabled: bool,
    pub channels: Vec<String>,
}

impl RuleSpec {
    /// Refuses what no evaluation could mean.
    pub fn validate(&self) -> Result<()> {
        let invalid = |message: String| Err(PanelError::invalid_argument(message));
        let name = self.name.trim();
        if name.is_empty() || name.len() > MAX_NAME || name != self.name {
            return invalid(format!(
                "a rule's name is 1 to {MAX_NAME} bytes without surrounding spaces"
            ));
        }
        if self.description.len() > MAX_DESCRIPTION {
            return invalid(format!(
                "a rule's description is at most {MAX_DESCRIPTION} bytes"
            ));
        }
        if !self.threshold.is_finite() || self.threshold < 0.0 {
            return invalid("the threshold is a finite number, not below 0".into());
        }
        if self.measure.is_ratio() && self.threshold > 1.0 {
            return invalid(format!(
                "{} is a share from 0 to 1, so its threshold is too",
                self.measure.name()
            ));
        }
        if self.pending_for > MAX_PENDING || self.pending_for.subsec_nanos() != 0 {
            return invalid("the pending period is whole seconds, at most a day".into());
        }
        let narrowed = self.site.is_some() || self.route.is_some();
        if narrowed && !self.measure.reads_requests() {
            return invalid(format!(
                "{} is not read by site or route",
                self.measure.name()
            ));
        }
        if self.route.is_some() && self.site.is_none() {
            return invalid("a route is read within its site".into());
        }
        if self.upstream.is_some() && self.measure != Measure::UpstreamErrorRatio {
            return invalid(format!("{} is not read by upstream", self.measure.name()));
        }
        if self.channels.len() > MAX_CHANNELS {
            return invalid(format!("a rule notifies at most {MAX_CHANNELS} channels"));
        }
        let mut channels = self.channels.clone();
        channels.sort();
        channels.dedup();
        if channels.len() != self.channels.len() {
            return invalid("a rule names each channel once".into());
        }
        for channel in &self.channels {
            identifier(channel, "a channel ID")?;
        }
        Ok(())
    }

    /// The condition, as people read it: `server_error_ratio above 0.05`.
    pub fn condition(&self) -> String {
        format!(
            "{} {} {}",
            self.measure.name(),
            self.comparison.name(),
            self.threshold
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> RuleSpec {
        RuleSpec {
            name: "Errors".into(),
            description: String::new(),
            measure: Measure::ServerErrorRatio,
            comparison: Comparison::Above,
            threshold: 0.05,
            pending_for: Duration::from_secs(300),
            site: Some(SiteId::new("shop").unwrap()),
            route: None,
            upstream: None,
            severity: Severity::Critical,
            enabled: true,
            channels: vec!["ops".into()],
        }
    }

    #[test]
    fn rules_mean_something_before_they_are_kept() {
        assert!(spec().validate().is_ok());
        let refused = |change: fn(&mut RuleSpec)| {
            let mut rule = spec();
            change(&mut rule);
            rule.validate().unwrap_err().message
        };
        assert!(refused(|rule| rule.threshold = 1.5).contains("share"));
        assert!(refused(|rule| rule.threshold = f64::NAN).contains("finite"));
        assert!(refused(|rule| rule.name = " Errors".into()).contains("name"));
        assert!(refused(|rule| rule.pending_for = Duration::from_millis(1500)).contains("whole"));
        assert!(refused(|rule| rule.measure = Measure::OpenConnections).contains("site"));
        assert!(refused(|rule| {
            rule.site = None;
            rule.route = Some(RouteId::new("api").unwrap());
        })
        .contains("within its site"));
        assert!(
            refused(|rule| rule.upstream = Some(UpstreamPoolId::new("app").unwrap()))
                .contains("upstream")
        );
        assert!(refused(|rule| rule.channels = vec!["ops".into(), "ops".into()]).contains("once"));
        assert!(identifier("bad id", "a rule ID").is_err());
    }

    #[test]
    fn comparisons_are_strict() {
        assert!(Comparison::Above.holds(0.06, 0.05));
        assert!(!Comparison::Above.holds(0.05, 0.05));
        assert!(Comparison::Below.holds(0.0, 1.0));
        assert_eq!(Measure::parse("latency_p95"), Some(Measure::LatencyP95));
        assert_eq!(Measure::parse("p95"), None);
    }
}
