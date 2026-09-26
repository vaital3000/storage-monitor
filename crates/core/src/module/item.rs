//! What a module found, and what it thinks of it.
//!
//! Everything here crosses IPC as the module produced it, so the wire shapes are part of the
//! contract, and the tests at the bottom pin them the way `action/model.rs` pins its own.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One thing a module found and can remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// `<module>:<native id>`, stable across discoveries. The selection of the Cleanup screen
    /// and every request of a batch are keyed by it, so a refresh keeps the ticks of the
    /// items that are still there.
    pub id: String,
    /// The id of the module that found it.
    pub module: String,
    /// The module's own word for it: `folder`, `object`, `worktree`, `image`.
    pub kind: String,
    pub title: String,
    pub subtitle: Option<String>,
    /// Where it lives, when it lives on the filesystem. Every item with a path offers
    /// "Reveal in Finder".
    pub path: Option<PathBuf>,
    pub size: Size,
    /// When it was last used, as well as the module can tell.
    pub last_used: Option<DateTime<Utc>>,
    pub verdict: Verdict,
    pub facts: Vec<Fact>,
    /// What can be done with it; the first is what a batch does unless told otherwise.
    pub actions: Vec<ActionSpec>,
}

impl Item {
    /// The action with this id, if the item offers it.
    pub fn action(&self, id: &str) -> Option<&ActionSpec> {
        self.actions.iter().find(|action| action.id == id)
    }
}

/// How much an item takes, and how sure the module is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Size {
    /// Allocated bytes, like the scanner's (`st_blocks * 512`).
    pub bytes: u64,
    /// The module could not measure it exactly — the layers a Docker image shares with
    /// others — and the screen draws it with a `~`.
    pub estimated: bool,
}

impl Size {
    pub fn exact(bytes: u64) -> Self {
        Self {
            bytes,
            estimated: false,
        }
    }

    pub fn estimated(bytes: u64) -> Self {
        Self {
            bytes,
            estimated: true,
        }
    }
}

/// How confident the module is that the item can go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// The module is confident it can go.
    Safe,
    /// Probably stale; a human should glance at it.
    Review,
    /// Not offered by default. The core refuses it unless one of the action's `force`
    /// options is turned on (ADR 0008).
    Keep,
}

/// A level and the reasons for it, in words a user reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub level: Level,
    pub reasons: Vec<Reason>,
}

impl Verdict {
    pub fn new(level: Level, reasons: Vec<Reason>) -> Self {
        Self { level, reasons }
    }
}

/// Why a verdict is what it is. The `code` is for tests and never shown; the `text` is the
/// sentence the screen shows, and a test that asserted on it would break with every edit
/// of the wording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reason {
    pub code: String,
    pub text: String,
}

impl Reason {
    pub fn new(code: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            text: text.into(),
        }
    }
}

/// One line of the detail panel: a label and a value the screen formats by its type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fact {
    pub key: String,
    pub label: String,
    pub value: FactValue,
}

impl Fact {
    pub fn new(key: impl Into<String>, label: impl Into<String>, value: FactValue) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            value,
        }
    }
}

/// A value the screen knows how to draw. Adjacently tagged — `{ "type": "bytes", "value":
/// 10 }` — because serde's internal tagging refuses a newtype variant that does not hold a
/// map, and every variant here holds a scalar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum FactValue {
    Text(String),
    /// Drawn 1000-based, like every size in the app.
    Bytes(u64),
    Count(u64),
    Date(DateTime<Utc>),
    Flag(bool),
    /// Drawn with "Reveal in Finder".
    Path(PathBuf),
}

/// Something that can be done with an item. What it does is not here: the module plans the
/// steps for a given mode when asked (ADR 0008), and the core derives from them everything
/// design section 7 listed as fields — whether it can go to the Trash, whether it can be
/// undone, and the exact preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionSpec {
    pub id: String,
    pub label: String,
    /// What the dialog promises and what the record says was freed. Two items of one batch
    /// must not count the same blocks: the property `Preview::total_bytes` asks of any
    /// source of sizes.
    pub estimated_free: u64,
    pub options: Vec<ActionOption>,
}

impl ActionSpec {
    /// The option with this id, if the action has it.
    pub fn option(&self, id: &str) -> Option<&ActionOption> {
        self.options.iter().find(|option| option.id == id)
    }
}

/// A switch on an action: "also delete the local branch", "force".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionOption {
    pub id: String,
    pub label: String,
    /// Whether it starts turned on.
    pub default: bool,
    /// Turning it on lets the action remove an item whose verdict is Keep.
    pub force: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item() -> Item {
        Item {
            id: "demo:keepsake".to_owned(),
            module: "demo".to_owned(),
            kind: "folder".to_owned(),
            title: "keepsake".to_owned(),
            subtitle: Some("3 files".to_owned()),
            path: Some(PathBuf::from("/h/demo/keepsake")),
            size: Size::exact(4096),
            last_used: Some(DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
            verdict: Verdict::new(
                Level::Keep,
                vec![Reason::new("marked-keep", "It holds a KEEP file")],
            ),
            facts: vec![Fact::new("files", "Files", FactValue::Count(3))],
            actions: vec![ActionSpec {
                id: "delete".to_owned(),
                label: "Delete folder".to_owned(),
                estimated_free: 4096,
                options: vec![ActionOption {
                    id: "force".to_owned(),
                    label: "Delete it even though it is marked keep".to_owned(),
                    default: false,
                    force: true,
                }],
            }],
        }
    }

    #[test]
    fn an_item_crosses_the_wire_in_camel_case_and_comes_back() {
        let json = serde_json::to_value(item()).unwrap();
        assert_eq!(
            json,
            json!({
                "id": "demo:keepsake",
                "module": "demo",
                "kind": "folder",
                "title": "keepsake",
                "subtitle": "3 files",
                "path": "/h/demo/keepsake",
                "size": { "bytes": 4096, "estimated": false },
                "lastUsed": "2023-11-14T22:13:20Z",
                "verdict": {
                    "level": "keep",
                    "reasons": [{ "code": "marked-keep", "text": "It holds a KEEP file" }],
                },
                "facts": [{ "key": "files", "label": "Files", "value": { "type": "count", "value": 3 } }],
                "actions": [{
                    "id": "delete",
                    "label": "Delete folder",
                    "estimatedFree": 4096,
                    "options": [{
                        "id": "force",
                        "label": "Delete it even though it is marked keep",
                        "default": false,
                        "force": true,
                    }],
                }],
            })
        );
        assert_eq!(serde_json::from_value::<Item>(json).unwrap(), item());
    }

    #[test]
    fn a_fact_value_is_tagged_by_type() {
        let cases = [
            (
                FactValue::Text("main".to_owned()),
                json!({ "type": "text", "value": "main" }),
            ),
            (
                FactValue::Bytes(10),
                json!({ "type": "bytes", "value": 10 }),
            ),
            (FactValue::Count(3), json!({ "type": "count", "value": 3 })),
            (
                FactValue::Date(DateTime::from_timestamp(0, 0).unwrap()),
                json!({ "type": "date", "value": "1970-01-01T00:00:00Z" }),
            ),
            (
                FactValue::Flag(true),
                json!({ "type": "flag", "value": true }),
            ),
            (
                FactValue::Path(PathBuf::from("/h/a")),
                json!({ "type": "path", "value": "/h/a" }),
            ),
        ];
        for (value, expected) in cases {
            assert_eq!(serde_json::to_value(&value).unwrap(), expected);
            assert_eq!(
                serde_json::from_value::<FactValue>(expected).unwrap(),
                value
            );
        }
    }

    #[test]
    fn a_level_is_a_camel_case_string() {
        assert_eq!(serde_json::to_value(Level::Safe).unwrap(), json!("safe"));
        assert_eq!(
            serde_json::to_value(Level::Review).unwrap(),
            json!("review")
        );
        assert_eq!(serde_json::to_value(Level::Keep).unwrap(), json!("keep"));
    }

    #[test]
    fn an_estimated_size_says_so() {
        assert_eq!(
            serde_json::to_value(Size::estimated(7)).unwrap(),
            json!({ "bytes": 7, "estimated": true })
        );
    }

    #[test]
    fn an_item_finds_its_actions_and_an_action_its_options() {
        let item = item();
        let action = item.action("delete").unwrap();
        assert!(action.option("force").unwrap().force);
        assert!(action.option("other").is_none());
        assert!(item.action("remove").is_none());
    }
}
