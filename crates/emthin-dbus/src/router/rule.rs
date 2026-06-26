//! Routing table — priority-ordered rules with glob-pattern matching
//! on destination, interface, and method fields.

use serde::{Deserialize, Serialize};

/// A single routing rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteRule {
    pub id: String,
    pub priority: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>, // glob pattern, None = wildcard
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    pub target: String, // "host" | "isolated" | "deny"
}

impl RouteRule {
    fn specificity(&self) -> u32 {
        let mut n = 0;
        if self.destination.is_some() {
            n += 1;
        }
        if self.interface.is_some() {
            n += 1;
        }
        if self.method.is_some() {
            n += 1;
        }
        n
    }

    /// Check whether this rule matches the given message fields.
    /// `destination`, `interface`, `member` come from the DBus message header.
    fn matches_str(
        &self,
        destination: Option<&str>,
        interface: Option<&str>,
        member: Option<&str>,
    ) -> bool {
        if let Some(ref pat) = self.destination {
            match destination {
                Some(d) => {
                    if !glob_match(pat, d) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        if let Some(ref iface) = self.interface {
            match interface {
                Some(i) => {
                    if i != iface {
                        return false;
                    }
                }
                None => return false,
            }
        }
        if let Some(ref m) = self.method {
            match member {
                Some(x) => {
                    if x != m {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }
}

/// Priority-ordered collection of [`RouteRule`]s.
#[derive(Debug, Clone)]
pub struct RoutingTable {
    rules: Vec<RouteRule>,
}

impl RoutingTable {
    pub fn new(rules: Vec<RouteRule>) -> Self {
        let mut t = Self { rules };
        t.sort();
        t
    }

    fn sort(&mut self) {
        self.rules.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| b.specificity().cmp(&a.specificity()))
        });
    }

    pub fn add(&mut self, rule: RouteRule) {
        self.rules.push(rule);
        self.sort();
    }

    pub fn remove(&mut self, id: &str) {
        self.rules.retain(|r| r.id != id);
    }

    /// Match message fields against the table. Returns the first matching
    /// rule's target, or `None` if no rule matches.
    pub fn route_str(
        &self,
        destination: Option<&str>,
        interface: Option<&str>,
        member: Option<&str>,
    ) -> Option<&str> {
        for rule in &self.rules {
            if rule.matches_str(destination, interface, member) {
                return Some(&rule.target);
            }
        }
        None
    }

    pub fn rules(&self) -> &[RouteRule] {
        &self.rules
    }

    pub fn into_rules(self) -> Vec<RouteRule> {
        self.rules
    }
}

impl Default for RoutingTable {
    fn default() -> Self {
        Self::new(vec![
            RouteRule {
                id: "builtin-portal".into(),
                priority: 100,
                destination: Some("org.freedesktop.portal.*".into()),
                interface: None,
                method: None,
                target: "host".into(),
            },
            RouteRule {
                id: "builtin-networkmanager".into(),
                priority: 100,
                destination: Some("org.freedesktop.NetworkManager".into()),
                interface: None,
                method: None,
                target: "host".into(),
            },
            RouteRule {
                id: "builtin-notifications".into(),
                priority: 100,
                destination: Some("org.freedesktop.Notifications".into()),
                interface: None,
                method: None,
                target: "host".into(),
            },
            RouteRule {
                id: "builtin-secrets".into(),
                priority: 100,
                destination: Some("org.freedesktop.Secrets".into()),
                interface: None,
                method: None,
                target: "host".into(),
            },
        ])
    }
}

/// Simple glob: `*` matches any sequence, `?` matches a single char.
fn glob_match(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    let mut pi = 0;
    let mut vi = 0;
    let mut star_pi: Option<usize> = None;
    let mut star_vi = 0;

    while vi < v.len() {
        if pi < p.len() && (p[pi] == v[vi] || p[pi] == '?') {
            pi += 1;
            vi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            star_vi = vi + 1;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            vi = star_vi;
            star_vi += 1;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matches_anything() {
        let table = RoutingTable::new(vec![RouteRule {
            id: "t1".into(),
            priority: 100,
            destination: None,
            interface: None,
            method: None,
            target: "host".into(),
        }]);
        assert_eq!(table.route_str(Some("org.chromium.X"), None, None), Some("host"));
    }

    #[test]
    fn destination_glob_matches_prefix() {
        let table = RoutingTable::new(vec![RouteRule {
            id: "t1".into(),
            priority: 100,
            destination: Some("org.freedesktop.portal.*".into()),
            interface: None,
            method: None,
            target: "host".into(),
        }]);
        assert_eq!(
            table.route_str(Some("org.freedesktop.portal.FileChooser"), None, None),
            Some("host")
        );
    }

    #[test]
    fn more_fields_wins_over_fewer() {
        let table = RoutingTable::new(vec![
            RouteRule {
                id: "broad".into(),
                priority: 100,
                destination: Some("org.example.*".into()),
                interface: None,
                method: None,
                target: "isolated".into(),
            },
            RouteRule {
                id: "specific".into(),
                priority: 100,
                destination: Some("org.example.Service".into()),
                interface: Some("org.example.Interface".into()),
                method: None,
                target: "host".into(),
            },
        ]);
        assert_eq!(
            table.route_str(Some("org.example.Service"), Some("org.example.Interface"), Some("DoThing")),
            Some("host")
        );
    }

    #[test]
    fn priority_overrides_field_count() {
        let table = RoutingTable::new(vec![
            RouteRule {
                id: "low".into(),
                priority: 50,
                destination: Some("org.example.*".into()),
                interface: Some("org.example.Iface".into()),
                method: None,
                target: "host".into(),
            },
            RouteRule {
                id: "high".into(),
                priority: 200,
                destination: Some("org.example.*".into()),
                interface: None,
                method: None,
                target: "isolated".into(),
            },
        ]);
        assert_eq!(
            table.route_str(Some("org.example.X"), Some("org.example.Iface"), Some("Y")),
            Some("isolated")
        );
    }

    #[test]
    fn no_match_returns_none() {
        let table = RoutingTable::new(vec![RouteRule {
            id: "p".into(),
            priority: 100,
            destination: Some("org.other.*".into()),
            interface: None,
            method: None,
            target: "host".into(),
        }]);
        assert_eq!(
            table.route_str(Some("org.unrelated.X"), Some("org.unrelated.Iface"), Some("Method")),
            None
        );
    }

    #[test]
    fn deny_target_returned_as_is() {
        let table = RoutingTable::new(vec![RouteRule {
            id: "block".into(),
            priority: 100,
            destination: Some("org.evil.*".into()),
            interface: None,
            method: None,
            target: "deny".into(),
        }]);
        assert_eq!(table.route_str(Some("org.evil.App"), None, None), Some("deny"));
    }

    #[test]
    fn glob_star_matches_subdomain() {
        assert!(glob_match("org.example.*", "org.example.ServiceSub"));
        assert!(!glob_match("org.example.*", "org.other.Service"));
    }

    #[test]
    fn glob_question_matches_single_char() {
        assert!(glob_match("org.example.???", "org.example.ABC"));
        assert!(!glob_match("org.example.???", "org.example.ABCD"));
    }
}
