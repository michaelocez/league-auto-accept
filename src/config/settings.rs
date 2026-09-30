//! Application settings: schema, defaults, normalisation, merge and migration.
//!
//! Ported from the reference implementation (`Electron V1/src/shared/settings.ts` and
//! `src/shared/contracts.ts`). The schema and semantics are preserved; the Rust rewrite owns its
//! own persistence format (decision A9), while normalisation still understands the legacy
//! `schemaVersion:1` `discordUserIds` array so an existing settings file can be migrated.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_DISCORD_USER_IDS: usize = 5;
pub const MAX_DISCORD_NICKNAME_LENGTH: usize = 40;
pub const MAX_WEBHOOK_URL_LENGTH: usize = 2048;
pub const MAX_NOTIFICATION_MESSAGE_LENGTH: usize = 1800;

/// Current settings schema version.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordMention {
    pub id: String,
    pub nickname: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationEvents {
    pub queue_popped: bool,
    pub auto_accepted: bool,
    pub game_started: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationMessages {
    pub queue_popped: String,
    pub auto_accepted: String,
    pub game_started: String,
}

/// The complete, typed application settings (schema v2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub schema_version: u32,
    pub auto_accept_enabled: bool,
    pub discord_notifications_enabled: bool,
    /// The only secret. Persisted separately, encrypted where the OS supports it.
    pub discord_webhook_url: String,
    pub discord_mentions: Vec<DiscordMention>,
    pub notification_events: NotificationEvents,
    pub notification_messages: NotificationMessages,
    pub minimize_to_tray: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            auto_accept_enabled: false,
            discord_notifications_enabled: false,
            discord_webhook_url: String::new(),
            discord_mentions: Vec::new(),
            notification_events: NotificationEvents {
                queue_popped: false,
                auto_accepted: true,
                game_started: false,
            },
            notification_messages: NotificationMessages {
                queue_popped: "{mentions} your League queue has popped.".into(),
                auto_accepted: "{mentions} the ready check was automatically accepted.".into(),
                game_started: "{mentions} the game is now in progress.".into(),
            },
            minimize_to_tray: true,
        }
    }
}

/// Normalises arbitrary JSON into valid settings, falling back per-field to `fallback`.
pub fn normalize_settings(value: &Value, fallback: &AppSettings) -> AppSettings {
    let source = value.as_object();

    let events = source
        .and_then(|object| object.get("notificationEvents"))
        .and_then(Value::as_object);
    let messages = source
        .and_then(|object| object.get("notificationMessages"))
        .and_then(Value::as_object);

    AppSettings {
        schema_version: SCHEMA_VERSION,
        auto_accept_enabled: boolean(source, "autoAcceptEnabled", fallback.auto_accept_enabled),
        discord_notifications_enabled: boolean(
            source,
            "discordNotificationsEnabled",
            fallback.discord_notifications_enabled,
        ),
        discord_webhook_url: string(
            source,
            "discordWebhookUrl",
            &fallback.discord_webhook_url,
            MAX_WEBHOOK_URL_LENGTH,
        )
        .trim()
        .to_string(),
        discord_mentions: normalize_mentions(source, fallback),
        notification_events: NotificationEvents {
            queue_popped: boolean(
                events,
                "queuePopped",
                fallback.notification_events.queue_popped,
            ),
            auto_accepted: boolean(
                events,
                "autoAccepted",
                fallback.notification_events.auto_accepted,
            ),
            game_started: boolean(
                events,
                "gameStarted",
                fallback.notification_events.game_started,
            ),
        },
        notification_messages: NotificationMessages {
            queue_popped: string(
                messages,
                "queuePopped",
                &fallback.notification_messages.queue_popped,
                MAX_NOTIFICATION_MESSAGE_LENGTH,
            ),
            auto_accepted: string(
                messages,
                "autoAccepted",
                &fallback.notification_messages.auto_accepted,
                MAX_NOTIFICATION_MESSAGE_LENGTH,
            ),
            game_started: string(
                messages,
                "gameStarted",
                &fallback.notification_messages.game_started,
                MAX_NOTIFICATION_MESSAGE_LENGTH,
            ),
        },
        minimize_to_tray: boolean(source, "minimizeToTray", fallback.minimize_to_tray),
    }
}

/// Merges a partial patch (arbitrary JSON) into `current`, then normalises.
///
/// `notificationEvents`/`notificationMessages` merge deeply so a partial patch does not wipe
/// sibling values. `schemaVersion` is ignored and always output as the current version.
pub fn merge_settings(current: &AppSettings, patch: &Value) -> AppSettings {
    let mut base = serde_json::to_value(current).unwrap_or(Value::Null);
    if let (Some(base_object), Some(patch_object)) = (base.as_object_mut(), patch.as_object()) {
        for (key, value) in patch_object {
            match key.as_str() {
                "schemaVersion" => {}
                "notificationEvents" | "notificationMessages" => {
                    if let (Some(destination), Some(source)) = (
                        base_object.get_mut(key).and_then(Value::as_object_mut),
                        value.as_object(),
                    ) {
                        for (nested_key, nested_value) in source {
                            destination.insert(nested_key.clone(), nested_value.clone());
                        }
                    }
                }
                _ => {
                    base_object.insert(key.clone(), value.clone());
                }
            }
        }
    }
    normalize_settings(&base, current)
}

fn normalize_mentions(
    source: Option<&serde_json::Map<String, Value>>,
    fallback: &AppSettings,
) -> Vec<DiscordMention> {
    let modern = source
        .and_then(|object| object.get("discordMentions"))
        .and_then(Value::as_array);
    let legacy = source
        .and_then(|object| object.get("discordUserIds"))
        .and_then(Value::as_array);

    let mut output: Vec<DiscordMention> = Vec::new();
    if let Some(entries) = modern {
        for entry in entries {
            let Some(record) = entry.as_object() else {
                continue;
            };
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("");
            let nickname = record
                .get("nickname")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("");
            push_mention(&mut output, id, nickname);
        }
    } else if let Some(ids) = legacy {
        for entry in ids {
            if let Some(id) = entry.as_str() {
                push_mention(&mut output, id.trim(), "");
            }
        }
    } else {
        return fallback.discord_mentions.clone();
    }
    output
}

fn push_mention(output: &mut Vec<DiscordMention>, id: &str, nickname: &str) {
    if output.len() >= MAX_DISCORD_USER_IDS {
        return;
    }
    if !is_valid_discord_id(id) {
        return;
    }
    if output.iter().any(|mention| mention.id == id) {
        return;
    }
    let nickname: String = nickname.chars().take(MAX_DISCORD_NICKNAME_LENGTH).collect();
    output.push(DiscordMention {
        id: id.to_string(),
        nickname,
    });
}

/// A Discord user ID is 17–20 ASCII digits (matches the reference validation).
pub fn is_valid_discord_id(value: &str) -> bool {
    let len = value.len();
    (17..=20).contains(&len) && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn boolean(source: Option<&serde_json::Map<String, Value>>, key: &str, fallback: bool) -> bool {
    source
        .and_then(|object| object.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(fallback)
}

fn string(
    source: Option<&serde_json::Map<String, Value>>,
    key: &str,
    fallback: &str,
    max_length: usize,
) -> String {
    match source
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
    {
        Some(value) => value.chars().take(max_length).collect(),
        None => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn uses_safe_defaults_for_malformed_settings() {
        let settings = normalize_settings(
            &json!({
                "autoAcceptEnabled": "yes",
                "discordUserIds": ["bad", "12345678901234567", "12345678901234567"]
            }),
            &AppSettings::default(),
        );
        assert!(!settings.auto_accept_enabled);
        assert_eq!(
            settings.discord_mentions,
            vec![DiscordMention {
                id: "12345678901234567".into(),
                nickname: String::new()
            }]
        );
        assert_eq!(settings.schema_version, SCHEMA_VERSION);
        assert!(settings.notification_events.auto_accepted);
    }

    #[test]
    fn deduplicates_and_caps_discord_mentions() {
        let entries: Vec<Value> = (1..=8)
            .map(|index| {
                let id = (1..=17)
                    .map(|_| char::from(b'0' + index))
                    .collect::<String>();
                json!({ "id": id, "nickname": format!(" Player {index} ") })
            })
            .collect();
        let settings = normalize_settings(
            &json!({ "discordMentions": entries }),
            &AppSettings::default(),
        );
        assert_eq!(settings.discord_mentions.len(), MAX_DISCORD_USER_IDS);
        assert_eq!(settings.discord_mentions[0].nickname, "Player 1");
    }

    #[test]
    fn migrates_legacy_user_id_arrays() {
        let settings = normalize_settings(
            &json!({ "schemaVersion": 1, "discordUserIds": ["12345678901234567"] }),
            &AppSettings::default(),
        );
        assert_eq!(settings.schema_version, SCHEMA_VERSION);
        assert_eq!(
            settings.discord_mentions,
            vec![DiscordMention {
                id: "12345678901234567".into(),
                nickname: String::new()
            }]
        );
    }

    #[test]
    fn merges_nested_patches_without_discarding_siblings() {
        let settings = merge_settings(
            &AppSettings::default(),
            &json!({ "notificationEvents": { "queuePopped": true } }),
        );
        assert!(settings.notification_events.queue_popped);
        assert!(settings.notification_events.auto_accepted);
        assert!(!settings.notification_events.game_started);
    }

    #[test]
    fn ignores_schema_version_in_patches_and_always_outputs_current() {
        let settings = merge_settings(
            &AppSettings::default(),
            &json!({ "schemaVersion": 99, "autoAcceptEnabled": true }),
        );
        assert_eq!(settings.schema_version, SCHEMA_VERSION);
        assert!(settings.auto_accept_enabled);
    }

    #[test]
    fn validates_discord_ids() {
        assert!(is_valid_discord_id("12345678901234567"));
        assert!(is_valid_discord_id("12345678901234567890"));
        assert!(!is_valid_discord_id("1234"));
        assert!(!is_valid_discord_id("1234567890123456a"));
    }
}
