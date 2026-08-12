//! Element attributes, including the class list.

use blitsen_dom::{DomBackend, DomError, DomName};

/// Attribute operations required by JavaScript element wrappers.
pub trait AttributeBackend {
    /// Stable node handle.
    type NodeId: Copy;

    /// Reads a non-namespaced HTML attribute.
    fn element_attribute(&self, node: Self::NodeId, name: &str)
    -> Result<Option<String>, DomError>;
    /// Sets a non-namespaced HTML attribute and invalidates selector matching.
    fn element_set_attribute(
        &mut self,
        node: Self::NodeId,
        name: &str,
        value: &str,
    ) -> Result<(), DomError>;
    /// Removes an attribute and invalidates selector matching when present.
    fn element_remove_attribute(
        &mut self,
        node: Self::NodeId,
        name: &str,
    ) -> Result<bool, DomError>;
}

impl<D: DomBackend> AttributeBackend for D {
    type NodeId = D::NodeId;

    fn element_attribute(
        &self,
        node: Self::NodeId,
        name: &str,
    ) -> Result<Option<String>, DomError> {
        self.attribute(node, &DomName::attribute(name.to_ascii_lowercase()))
    }

    fn element_set_attribute(
        &mut self,
        node: Self::NodeId,
        name: &str,
        value: &str,
    ) -> Result<(), DomError> {
        self.set_attribute(node, &DomName::attribute(name.to_ascii_lowercase()), value)
    }

    fn element_remove_attribute(
        &mut self,
        node: Self::NodeId,
        name: &str,
    ) -> Result<bool, DomError> {
        self.remove_attribute(node, &DomName::attribute(name.to_ascii_lowercase()))
    }
}

/// Runtime-neutral attributes and `classList` implementation.
pub struct ElementAttributesApi<'a, D: AttributeBackend> {
    backend: &'a mut D,
    node: D::NodeId,
}

impl<'a, D: AttributeBackend> ElementAttributesApi<'a, D> {
    /// Wraps an element from the authoritative backend.
    pub fn new(backend: &'a mut D, node: D::NodeId) -> Self {
        Self { backend, node }
    }

    /// Implements `getAttribute`.
    pub fn get_attribute(&self, name: &str) -> Result<Option<String>, DomError> {
        self.backend.element_attribute(self.node, name)
    }

    /// Implements `setAttribute`.
    pub fn set_attribute(&mut self, name: &str, value: &str) -> Result<(), DomError> {
        self.backend.element_set_attribute(self.node, name, value)
    }

    /// Implements `removeAttribute`.
    pub fn remove_attribute(&mut self, name: &str) -> Result<(), DomError> {
        self.backend.element_remove_attribute(self.node, name)?;
        Ok(())
    }

    /// Implements `hasAttribute`.
    pub fn has_attribute(&self, name: &str) -> Result<bool, DomError> {
        Ok(self.get_attribute(name)?.is_some())
    }

    /// Implements the reflected `id` getter.
    pub fn id(&self) -> Result<String, DomError> {
        Ok(self.get_attribute("id")?.unwrap_or_default())
    }

    /// Implements the reflected `id` setter.
    pub fn set_id(&mut self, value: &str) -> Result<(), DomError> {
        self.set_attribute("id", value)
    }

    /// Implements the reflected `className` getter.
    pub fn class_name(&self) -> Result<String, DomError> {
        Ok(self.get_attribute("class")?.unwrap_or_default())
    }

    /// Implements the reflected `className` setter.
    pub fn set_class_name(&mut self, value: &str) -> Result<(), DomError> {
        self.set_attribute("class", value)
    }

    /// Implements `classList.contains`.
    pub fn class_contains(&self, token: &str) -> Result<bool, DomError> {
        validate_class_token(token)?;
        Ok(self.class_tokens()?.iter().any(|class| class == token))
    }

    /// Implements `classList.add` for one or more tokens.
    pub fn class_add(&mut self, tokens: &[&str]) -> Result<(), DomError> {
        validate_class_tokens(tokens)?;
        let mut classes = self.class_tokens()?;
        for token in tokens {
            if !classes.iter().any(|class| class == token) {
                classes.push((*token).into());
            }
        }
        self.write_class_tokens(classes)
    }

    /// Implements `classList.remove` for one or more tokens.
    pub fn class_remove(&mut self, tokens: &[&str]) -> Result<(), DomError> {
        validate_class_tokens(tokens)?;
        let mut classes = self.class_tokens()?;
        classes.retain(|class| !tokens.iter().any(|token| class == token));
        self.write_class_tokens(classes)
    }

    /// Implements `classList.toggle`, including its optional force argument.
    pub fn class_toggle(&mut self, token: &str, force: Option<bool>) -> Result<bool, DomError> {
        validate_class_token(token)?;
        let present = self.class_contains(token)?;
        let desired = force.unwrap_or(!present);
        if desired != present {
            if desired {
                self.class_add(&[token])?;
            } else {
                self.class_remove(&[token])?;
            }
        }
        Ok(desired)
    }

    fn class_tokens(&self) -> Result<Vec<String>, DomError> {
        Ok(self
            .class_name()?
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect())
    }

    fn write_class_tokens(&mut self, classes: Vec<String>) -> Result<(), DomError> {
        self.set_class_name(&classes.join(" "))
    }
}

fn validate_class_tokens(tokens: &[&str]) -> Result<(), DomError> {
    for token in tokens {
        validate_class_token(token)?;
    }
    Ok(())
}

fn validate_class_token(token: &str) -> Result<(), DomError> {
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        Err(DomError::Syntax(
            "class token must be non-empty and contain no whitespace".into(),
        ))
    } else {
        Ok(())
    }
}
