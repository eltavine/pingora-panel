use panel_errors::{PanelError, Result};
use std::{ffi::OsString, net::SocketAddr, time::Duration};

type Lookup<'a> = Box<dyn FnMut(&str) -> Option<OsString> + 'a>;

/// Reads process settings through an injected lookup, so tests and other
/// configuration sources never mutate the process environment.
pub struct Environment<'a> {
    lookup: Lookup<'a>,
}

impl Environment<'static> {
    pub fn process() -> Self {
        Self::from_lookup(|name| std::env::var_os(name))
    }
}

impl<'a> Environment<'a> {
    pub fn from_lookup(lookup: impl FnMut(&str) -> Option<OsString> + 'a) -> Self {
        Self {
            lookup: Box::new(lookup),
        }
    }

    /// A non-empty UTF-8 value, or `None` when unset or empty.
    pub fn string(&mut self, name: &str) -> Result<Option<String>> {
        let Some(value) = (self.lookup)(name) else {
            return Ok(None);
        };
        let value = value
            .into_string()
            .map_err(|_| PanelError::invalid_argument(format!("{name} must be valid UTF-8")))?;
        Ok((!value.is_empty()).then_some(value))
    }

    pub fn required(&mut self, name: &str) -> Result<String> {
        self.string(name)?
            .ok_or_else(|| PanelError::invalid_argument(format!("{name} is required")))
    }

    /// A secret from `name`, or from the file named by `<name>_FILE` as
    /// container secrets are mounted. One trailing newline is removed from
    /// file contents. Setting both is ambiguous and rejected.
    pub fn secret(&mut self, name: &str) -> Result<Option<String>> {
        let file_name = format!("{name}_FILE");
        match (self.string(name)?, self.string(&file_name)?) {
            (Some(_), Some(_)) => Err(PanelError::invalid_argument(format!(
                "set either {name} or {file_name}, not both"
            ))),
            (Some(value), None) => Ok(Some(value)),
            (None, Some(path)) => {
                let mut value = std::fs::read_to_string(&path).map_err(|error| {
                    PanelError::invalid_argument(format!("{file_name} cannot be read: {error}"))
                })?;
                if value.ends_with('\n') {
                    value.pop();
                    if value.ends_with('\r') {
                        value.pop();
                    }
                }
                Ok((!value.is_empty()).then_some(value))
            }
            (None, None) => Ok(None),
        }
    }

    pub fn socket_addr(&mut self, name: &str, default: SocketAddr) -> Result<SocketAddr> {
        self.string(name)?.map_or(Ok(default), |value| {
            value
                .parse()
                .map_err(|error| PanelError::invalid_argument(format!("invalid {name}: {error}")))
        })
    }

    /// A duration given in whole milliseconds; zero is rejected.
    pub fn millis(&mut self, name: &str, default: Duration) -> Result<Duration> {
        let Some(value) = self.string(name)? else {
            return Ok(default);
        };
        match value.parse::<u64>() {
            Ok(0) | Err(_) => Err(PanelError::invalid_argument(format!(
                "{name} must be a positive number of milliseconds"
            ))),
            Ok(millis) => Ok(Duration::from_millis(millis)),
        }
    }
}

/// Plaintext listeners stay on loopback until internal transports are
/// authenticated; a remote bind fails at startup instead of degrading
/// silently.
pub fn require_loopback(name: &str, address: SocketAddr) -> Result<SocketAddr> {
    if address.ip().is_loopback() {
        Ok(address)
    } else {
        Err(PanelError::invalid_argument(format!(
            "{name} must be a loopback address until internal transports are authenticated"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn environment(values: &[(&str, &str)]) -> Environment<'static> {
        let values: HashMap<String, OsString> = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
            .collect();
        Environment::from_lookup(move |name| values.get(name).cloned())
    }

    #[test]
    fn values_parse_with_defaults_and_reject_invalid_input() {
        let default = "127.0.0.1:9000".parse().unwrap();
        let mut env = environment(&[
            ("ADDR", "127.0.0.1:9100"),
            ("EMPTY", ""),
            ("BAD_ADDR", "localhost"),
            ("INTERVAL", "250"),
            ("ZERO", "0"),
        ]);
        assert_eq!(env.socket_addr("ADDR", default).unwrap().port(), 9100);
        assert_eq!(env.socket_addr("MISSING", default).unwrap(), default);
        assert!(env.socket_addr("BAD_ADDR", default).is_err());
        assert_eq!(env.string("EMPTY").unwrap(), None);
        assert!(env.required("EMPTY").is_err());
        assert_eq!(
            env.millis("INTERVAL", Duration::from_secs(1)).unwrap(),
            Duration::from_millis(250)
        );
        assert!(env.millis("ZERO", Duration::from_secs(1)).is_err());
    }

    #[test]
    fn secrets_come_from_values_or_files_but_not_both() {
        let directory = std::env::temp_dir().join(format!("panel-env-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("password");
        std::fs::write(&file, "s3cret\n").unwrap();
        let path = file.to_str().unwrap();

        let mut env = environment(&[("FROM_FILE_FILE", path), ("DIRECT", "inline")]);
        assert_eq!(env.secret("FROM_FILE").unwrap().as_deref(), Some("s3cret"));
        assert_eq!(env.secret("DIRECT").unwrap().as_deref(), Some("inline"));
        assert_eq!(env.secret("ABSENT").unwrap(), None);

        let mut both = environment(&[("BOTH", "inline"), ("BOTH_FILE", path)]);
        assert!(both.secret("BOTH").is_err());
        let mut missing = environment(&[("GONE_FILE", "/nonexistent/secret")]);
        assert!(missing.secret("GONE").is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn plaintext_listeners_are_loopback_only() {
        assert!(require_loopback("ADDR", "127.0.0.1:1".parse().unwrap()).is_ok());
        assert!(require_loopback("ADDR", "[::1]:1".parse().unwrap()).is_ok());
        assert!(require_loopback("ADDR", "0.0.0.0:1".parse().unwrap()).is_err());
        assert!(require_loopback("ADDR", "192.0.2.1:1".parse().unwrap()).is_err());
    }
}
