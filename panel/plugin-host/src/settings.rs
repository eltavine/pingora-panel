//! A plugin's settings: a JSON object its manifest's JSON Schema accepts,
//! whose secret references name secrets instead of holding them (ADR 0044).

use crate::manifest::is_name;
use plugin_contracts::SECRET_REFERENCE_FORMAT;
use serde_json::Value;

/// The manifest's configuration schema; `None` when the plugin takes no
/// settings.
pub fn schema(text: &str) -> Result<Option<Value>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let schema: Value = serde_json::from_str(text)
        .map_err(|error| format!("the configuration schema is not JSON: {error}"))?;
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err("the configuration schema does not describe an object".into());
    }
    jsonschema::validator_for(&schema)
        .map_err(|error| format!("the configuration schema is not a JSON Schema: {error}"))?;
    Ok(Some(schema))
}

/// What is wrong with `settings` for the plugin's schema.
pub fn problems(schema: Option<&Value>, settings: &Value) -> Vec<String> {
    let Some(schema) = schema else {
        return if settings.as_object().is_some_and(serde_json::Map::is_empty) {
            Vec::new()
        } else {
            vec!["the plugin takes no settings".into()]
        };
    };
    let validator = match jsonschema::validator_for(schema) {
        Ok(validator) => validator,
        Err(error) => {
            return vec![format!(
                "the configuration schema is not a JSON Schema: {error}"
            )]
        }
    };
    let mut found: Vec<String> = validator
        .iter_errors(settings)
        .map(|error| {
            let path = error.instance_path().to_string();
            if path.is_empty() {
                error.to_string()
            } else {
                format!("{path}: {error}")
            }
        })
        .collect();
    for (pointer, reference) in references(schema, settings) {
        if let Err(problem) = Reference::parse(&reference) {
            found.push(format!("{pointer}: {problem}"));
        }
    }
    found
}

/// A secret a setting names: one sealed in the host's store, or one a
/// secret-provider plugin returns.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reference {
    Vault(String),
    Plugin { plugin: String, path: String },
}

impl Reference {
    pub fn parse(text: &str) -> Result<Self, String> {
        let usage = || {
            format!("{text:?} is not a secret reference such as vault:<name> or <plugin>:<path>")
        };
        let (scheme, rest) = text.split_once(':').ok_or_else(usage)?;
        if rest.is_empty() {
            return Err(usage());
        }
        if scheme == "vault" {
            return if is_name(rest) {
                Ok(Self::Vault(rest.to_owned()))
            } else {
                Err(format!(
                    "{rest:?} is not a secret name: lowercase letters, digits and hyphens"
                ))
            };
        }
        if is_name(scheme) {
            Ok(Self::Plugin {
                plugin: scheme.to_owned(),
                path: rest.to_owned(),
            })
        } else {
            Err(usage())
        }
    }
}

/// The JSON pointers and texts of the settings' secret references: strings
/// whose own schema says `"format": "secret-reference"`, through
/// `properties` and `items`.
pub fn references(schema: &Value, settings: &Value) -> Vec<(String, String)> {
    let mut found = Vec::new();
    walk(schema, settings, String::new(), &mut found);
    found
}

fn walk(schema: &Value, value: &Value, pointer: String, found: &mut Vec<(String, String)>) {
    if schema.get("format").and_then(Value::as_str) == Some(SECRET_REFERENCE_FORMAT) {
        if let Some(text) = value.as_str() {
            found.push((pointer.clone(), text.to_owned()));
        }
    }
    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) {
        for (key, property) in properties {
            if let Some(inner) = object.get(key) {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                walk(property, inner, format!("{pointer}/{escaped}"), found);
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (index, inner) in array.iter().enumerate() {
            walk(items, inner, format!("{pointer}/{index}"), found);
        }
    }
}

/// The settings with each reference at a pointer replaced by its value.
pub fn resolved(settings: &Value, values: &[(String, String)]) -> Value {
    let mut resolved = settings.clone();
    for (pointer, value) in values {
        if let Some(slot) = resolved.pointer_mut(pointer) {
            *slot = Value::String(value.clone());
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "type": "object",
            "required": ["zone"],
            "additionalProperties": false,
            "properties": {
                "zone": {"type": "string"},
                "token": {"type": "string", "format": "secret-reference"},
                "servers": {"type": "array", "items": {
                    "type": "object",
                    "properties": {"key": {"type": "string", "format": "secret-reference"}}
                }}
            }
        })
    }

    #[test]
    fn schemas_describe_objects() {
        assert_eq!(schema("").unwrap(), None);
        assert!(schema(&sample().to_string()).unwrap().is_some());
        assert!(schema("{").unwrap_err().contains("not JSON"));
        assert!(schema(r#"{"type": "string"}"#)
            .unwrap_err()
            .contains("an object"));
        assert!(schema(r#"{"type": "object", "minProperties": "two"}"#)
            .unwrap_err()
            .contains("not a JSON Schema"));
    }

    #[test]
    fn settings_follow_the_schema_and_name_secrets_properly() {
        let schema = sample();
        let settings = json!({
            "zone": "shop.example.",
            "token": "vault:dns-token",
            "servers": [{"key": "vault-plugin:kv/dns"}]
        });
        assert_eq!(problems(Some(&schema), &settings), Vec::<String>::new());
        assert_eq!(
            references(&schema, &settings),
            vec![
                (
                    "/servers/0/key".to_owned(),
                    "vault-plugin:kv/dns".to_owned()
                ),
                ("/token".to_owned(), "vault:dns-token".to_owned()),
            ]
        );
        let found = problems(Some(&schema), &json!({"token": "plain-secret", "extra": 1}));
        assert!(
            found.iter().any(|problem| problem.contains("zone")),
            "{found:#?}"
        );
        assert!(
            found.iter().any(|problem| problem.contains("extra")),
            "{found:#?}"
        );
        assert!(
            found
                .iter()
                .any(|problem| problem
                    .starts_with("/token: \"plain-secret\" is not a secret reference")),
            "{found:#?}"
        );
        assert_eq!(
            problems(None, &json!({"zone": "x"})),
            vec!["the plugin takes no settings".to_owned()]
        );
        assert!(problems(None, &json!({})).is_empty());
    }

    #[test]
    fn references_parse_and_resolve() {
        assert_eq!(
            Reference::parse("vault:dns-token").unwrap(),
            Reference::Vault("dns-token".into())
        );
        assert_eq!(
            Reference::parse("hashicorp-vault:secret/data/dns").unwrap(),
            Reference::Plugin {
                plugin: "hashicorp-vault".into(),
                path: "secret/data/dns".into()
            }
        );
        for broken in ["vault:", "token", "Vault:x", "vault:Bad_Name"] {
            assert!(Reference::parse(broken).is_err(), "{broken}");
        }
        let settings = json!({"zone": "shop.example.", "token": "vault:dns-token"});
        assert_eq!(
            resolved(&settings, &[("/token".into(), "s3cret".into())]),
            json!({"zone": "shop.example.", "token": "s3cret"})
        );
    }
}
