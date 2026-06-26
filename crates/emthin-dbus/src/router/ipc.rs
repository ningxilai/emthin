//! JSON-RPC IPC messages between the emthin main process and the
//! emthin-dbus-router subprocess.

use serde::{Deserialize, Serialize};

use crate::fcitx::FcitxEvent;
use super::rule::RouteRule;

/// Requests sent from emthin main → router.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum RouterRequest {
    AddRule { rule: RouteRule },
    RemoveRule { id: String },
    ListRules,
    #[serde(rename = "ime_commit")]
    ImeCommit { ic_path: String, text: String },
    #[serde(rename = "ime_preedit")]
    ImePreedit {
        ic_path: String,
        text: String,
        cursor_begin: i32,
        cursor_end: i32,
    },
}

/// Notifications sent from router → emthin main.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum RouterNotification {
    #[serde(rename = "fcitx_event")]
    FcitxEvent(FcitxEvent),
    #[serde(rename = "rule_added")]
    RuleAdded {
        id: String,
        rule: RouteRule,
    },
    #[serde(rename = "rule_removed")]
    RuleRemoved { id: String },
}
