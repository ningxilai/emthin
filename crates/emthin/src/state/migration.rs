use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationPolicy {
    Manual,
    ByWorkspaceAffinity,
}

impl fmt::Display for MigrationPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manual => f.write_str("manual"),
            Self::ByWorkspaceAffinity => f.write_str("by_workspace_affinity"),
        }
    }
}

impl FromStr for MigrationPolicy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "manual" => Ok(Self::Manual),
            "by_workspace_affinity" => Ok(Self::ByWorkspaceAffinity),
            other => Err(format!("unknown migration policy: {other}")),
        }
    }
}
