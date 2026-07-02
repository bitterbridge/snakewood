use std::fmt;

use serde::{Deserialize, Serialize};

/// A validated, human-readable, namespaced identifier, e.g. `snakewood/clearing`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct EntityId(String);

#[derive(Debug, PartialEq)]
pub enum IdError {
    Empty,
    InvalidChar(char),
    NoNamespace,
    LeadingOrTrailingSlash,
}

impl EntityId {
    pub fn new(s: impl Into<String>) -> Result<EntityId, IdError> {
        let s = s.into();
        if s.is_empty() {
            return Err(IdError::Empty);
        }
        if s.starts_with('/') || s.ends_with('/') {
            return Err(IdError::LeadingOrTrailingSlash);
        }
        for c in s.chars() {
            let ok = c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '/' | '_' | '-');
            if !ok {
                return Err(IdError::InvalidChar(c));
            }
        }
        if !s.contains('/') {
            return Err(IdError::NoNamespace);
        }
        Ok(EntityId(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn zone(&self) -> &str {
        self.0.split('/').next().unwrap_or(&self.0)
    }

    pub fn name(&self) -> &str {
        match self.0.split_once('/') {
            Some((_, rest)) => rest,
            None => &self.0,
        }
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_namespaced_id() {
        let id = EntityId::new("snakewood/clearing").unwrap();
        assert_eq!(id.as_str(), "snakewood/clearing");
        assert_eq!(id.zone(), "snakewood");
        assert_eq!(id.name(), "clearing");
    }

    #[test]
    fn name_keeps_deeper_segments() {
        let id = EntityId::new("snakewood/mob/goblin").unwrap();
        assert_eq!(id.zone(), "snakewood");
        assert_eq!(id.name(), "mob/goblin");
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(EntityId::new(""), Err(IdError::Empty));
    }

    #[test]
    fn rejects_missing_namespace() {
        assert_eq!(EntityId::new("clearing"), Err(IdError::NoNamespace));
    }

    #[test]
    fn rejects_uppercase() {
        assert_eq!(
            EntityId::new("Snakewood/clearing"),
            Err(IdError::InvalidChar('S'))
        );
    }

    #[test]
    fn rejects_leading_or_trailing_slash() {
        assert_eq!(
            EntityId::new("/snakewood"),
            Err(IdError::LeadingOrTrailingSlash)
        );
        assert_eq!(
            EntityId::new("snakewood/"),
            Err(IdError::LeadingOrTrailingSlash)
        );
    }
}
