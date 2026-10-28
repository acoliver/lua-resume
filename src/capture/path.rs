//! The path from the captured coroutine to a value, for error messages.
//!
//! Paths are shared linked nodes, so a long chain of tables costs one node per table.

use std::rc::Rc;

/// A position in the captured data. Rendering joins the labels from the root with ` > `.
#[derive(Clone, Debug)]
pub(super) struct Path(Rc<Node>);

#[derive(Debug)]
struct Node {
    parent: Option<Path>,
    label: String,
}

impl Path {
    pub(super) fn root(label: &str) -> Self {
        Self(Rc::new(Node {
            parent: None,
            label: label.to_owned(),
        }))
    }

    pub(super) fn child(&self, label: String) -> Self {
        Self(Rc::new(Node {
            parent: Some(self.clone()),
            label,
        }))
    }

    pub(super) fn render(&self) -> String {
        let mut labels = Vec::new();
        let mut node = Some(self);
        while let Some(path) = node {
            labels.push(path.0.label.as_str());
            node = path.0.parent.as_ref();
        }
        labels.reverse();
        labels.join(" > ")
    }
}

impl Drop for Node {
    /// Frees a long chain without recursing once per node.
    fn drop(&mut self) {
        let mut next = self.parent.take();
        while let Some(path) = next {
            match Rc::try_unwrap(path.0) {
                Ok(mut node) => next = node.parent.take(),
                Err(_) => break,
            }
        }
    }
}
