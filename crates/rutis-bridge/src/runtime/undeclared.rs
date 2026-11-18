//! What a host does with the services a runtime row provides without
//! declaring them ("announced", `rows.announce`): Cordis publishes each one
//! under the key the row's projection gives it, and a host that opens
//! itself to third parties may refuse them or allow only some (#1040).

/// The host's policy for the services a row announces without declaring
/// them. Declared services are not affected: a row's `provides` and its
/// `isolate` keep the keys and the rules they have.
///
/// The default, [`UndeclaredPolicy::Publish`], is Cordis's behaviour: a
/// name the row did not declare is published like a declared one, and the
/// first provider wins. A host that loads plugins it did not write sets
/// this on the runtime's session (`SpawnPlugin::undeclared`,
/// `Session::undeclared`) or on a mount (`Mount::undeclared`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum UndeclaredPolicy {
    /// Publish every announced name, as Cordis does.
    #[default]
    Publish,
    /// Refuse every announced name: it is not published, and the row fails.
    Refuse,
    /// Publish only the names listed; refuse every other, failing the row.
    /// An empty list is [`UndeclaredPolicy::Refuse`].
    Allow(Vec<String>),
}

impl UndeclaredPolicy {
    /// [`UndeclaredPolicy::Allow`] from any list of names.
    pub fn allow(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::Allow(names.into_iter().map(Into::into).collect())
    }

    /// Whether the announced `name` may be published.
    pub fn permits(&self, name: &str) -> bool {
        match self {
            Self::Publish => true,
            Self::Refuse => false,
            Self::Allow(allowed) => allowed.iter().any(|allowed| allowed == name),
        }
    }

    /// Why the row `row` fails when it announces `name`: `None` when the
    /// policy permits it.
    pub(crate) fn refusal(&self, row: &str, name: &str) -> Option<String> {
        if self.permits(name) {
            return None;
        }
        Some(match self {
            Self::Allow(allowed) if !allowed.is_empty() => format!(
                "{name} is provided by row {row} without declaring it; the host lets a runtime \
                 publish only {}",
                allowed.join(", ")
            ),
            _ => format!(
                "{name} is provided by row {row} without declaring it; the host does not let a \
                 runtime publish names it does not declare"
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// guarantee: loader/announced-services
    #[test]
    fn the_default_publishes_every_name() {
        assert_eq!(UndeclaredPolicy::default(), UndeclaredPolicy::Publish);
        assert!(UndeclaredPolicy::Publish.permits("llm"));
        assert_eq!(UndeclaredPolicy::Publish.refusal("row w", "llm"), None);
    }

    /// guarantee: loader/announced-services
    #[test]
    fn refuse_and_allow_say_why_a_name_is_not_published() {
        assert!(!UndeclaredPolicy::Refuse.permits("llm"));
        assert_eq!(
            UndeclaredPolicy::Refuse.refusal("w", "llm").as_deref(),
            Some(
                "llm is provided by row w without declaring it; the host does not let a runtime \
                 publish names it does not declare"
            )
        );
        let allowed = UndeclaredPolicy::allow(["tools", "notes"]);
        assert!(allowed.permits("tools") && allowed.permits("notes"));
        assert!(!allowed.permits("llm"));
        assert_eq!(
            allowed.refusal("w", "llm").as_deref(),
            Some(
                "llm is provided by row w without declaring it; the host lets a runtime publish \
                 only tools, notes"
            )
        );
        assert!(!UndeclaredPolicy::Allow(Vec::new()).permits("llm"));
        assert_eq!(
            UndeclaredPolicy::Allow(Vec::new()).refusal("w", "llm"),
            UndeclaredPolicy::Refuse.refusal("w", "llm")
        );
    }
}
