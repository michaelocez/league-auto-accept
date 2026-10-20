//! Application settings: schema, defaults, normalisation, merge and migration.
//!
//! Persistence has its own format, while normalisation still understands the legacy
//! `schemaVersion:1` `discordUserIds` array so an existing settings file can be migrated.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::notifications::discord::MAX_TEMPLATE_LENGTH;

pub const MAX_DISCORD_USER_IDS: usize = 5;
pub const MAX_DISCORD_NICKNAME_LENGTH: usize = 40;

/// A length limit for one stored settings field.
///
/// Passing the limit as a named choice rather than a plain number is what keeps a call site from
/// silently enforcing a different cap than it means to: the three message templates are normalised
/// with exactly the limit the editor enforces and the sender accepts, and that is the whole point
/// of the type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoredLength {
    /// A notification message template, capped at [`MAX_TEMPLATE_LENGTH`].
    Template,
    /// A mention's displayed nickname, capped at [`MAX_DISCORD_NICKNAME_LENGTH`].
    Nickname,
}

impl StoredLength {
    /// The cap itself, in characters, for comparing against a `chars().count()`.
    fn chars(self) -> usize {
        match self {
            Self::Template => MAX_TEMPLATE_LENGTH,
            Self::Nickname => MAX_DISCORD_NICKNAME_LENGTH,
        }
    }
}

/// Current settings schema version.
pub const SCHEMA_VERSION: u32 = 3;

/// Light or dark appearance, persisted so a restart keeps the chosen theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

/// Window backdrop material, persisted so a restart keeps the chosen glass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackdropMode {
    Opaque,
    Mica,
    /// The default: a translucent window over the desktop.
    #[default]
    Acrylic,
}

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

/// The complete, typed application settings (schema v3).
///
/// The Discord webhook — the only secret — is deliberately absent: it is owned by the settings
/// store and the notification sender, never by application state, the tray or the UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub schema_version: u32,
    pub auto_accept_enabled: bool,
    pub discord_notifications_enabled: bool,
    pub discord_mentions: Vec<DiscordMention>,
    pub notification_events: NotificationEvents,
    pub notification_messages: NotificationMessages,
    pub minimize_to_tray: bool,
    pub theme_mode: ThemeMode,
    pub window_backdrop: BackdropMode,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            auto_accept_enabled: false,
            discord_notifications_enabled: false,
            discord_mentions: Vec::new(),
            notification_events: NotificationEvents {
                queue_popped: false,
                auto_accepted: true,
                game_started: false,
            },
            notification_messages: NotificationMessages {
                queue_popped: "{mentions} Match has been found".into(),
                auto_accepted: "{mentions} the match has been accepted".into(),
                game_started: "{mentions} the game is now in progress.".into(),
            },
            minimize_to_tray: true,
            theme_mode: ThemeMode::Dark,
            // Acrylic by default: it blurs what is behind the window, which is the see-through glass
            // the design aims at. Mica (subtler, wallpaper-sampled) and Opaque stay available, and an
            // existing install keeps whatever it has already saved.
            window_backdrop: BackdropMode::Acrylic,
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
                StoredLength::Template,
            ),
            auto_accepted: string(
                messages,
                "autoAccepted",
                &fallback.notification_messages.auto_accepted,
                StoredLength::Template,
            ),
            game_started: string(
                messages,
                "gameStarted",
                &fallback.notification_messages.game_started,
                StoredLength::Template,
            ),
        },
        minimize_to_tray: boolean(source, "minimizeToTray", fallback.minimize_to_tray),
        theme_mode: theme_mode(source, "themeMode", fallback.theme_mode),
        window_backdrop: backdrop_mode(source, "windowBackdrop", fallback.window_backdrop),
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
    let nickname: String = nickname
        .chars()
        .take(StoredLength::Nickname.chars())
        .collect();
    output.push(DiscordMention {
        id: id.to_string(),
        nickname,
    });
}

/// The mention ids a notification may actually use: trimmed, valid, de-duplicated and capped at
/// [`MAX_DISCORD_USER_IDS`].
///
/// Normalisation applies these rules when settings are written; the notification sender applies
/// them again so a notification can never mention more people than the cap.
pub fn allowed_mention_ids(mentions: &[DiscordMention]) -> Vec<String> {
    let mut output: Vec<String> = Vec::new();
    for mention in mentions {
        if output.len() >= MAX_DISCORD_USER_IDS {
            break;
        }
        let id = mention.id.trim();
        if !is_valid_discord_id(id) || output.iter().any(|existing| existing == id) {
            continue;
        }
        output.push(id.to_string());
    }
    output
}

/// A Discord user ID is 17–20 ASCII digits.
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

fn theme_mode(
    source: Option<&serde_json::Map<String, Value>>,
    key: &str,
    fallback: ThemeMode,
) -> ThemeMode {
    match source
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
    {
        Some("light") => ThemeMode::Light,
        Some("dark") => ThemeMode::Dark,
        _ => fallback,
    }
}

fn backdrop_mode(
    source: Option<&serde_json::Map<String, Value>>,
    key: &str,
    fallback: BackdropMode,
) -> BackdropMode {
    match source
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
    {
        Some("mica") => BackdropMode::Mica,
        Some("acrylic") => BackdropMode::Acrylic,
        Some("opaque") => BackdropMode::Opaque,
        _ => fallback,
    }
}

fn string(
    source: Option<&serde_json::Map<String, Value>>,
    key: &str,
    fallback: &str,
    max_length: StoredLength,
) -> String {
    match source
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
    {
        Some(value) => value.chars().take(max_length.chars()).collect(),
        None => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::discord::{MAX_CONTENT_LENGTH, MAX_MENTION_EXPANSION_LENGTH};
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

    #[test]
    fn allowlisted_mention_ids_are_valid_unique_and_capped() {
        let mentions: Vec<DiscordMention> = (0..8)
            .map(|index| DiscordMention {
                id: format!("{:017}", index + 1),
                nickname: format!("Player {}", index + 1),
            })
            .collect();
        let ids = allowed_mention_ids(&mentions);
        assert_eq!(ids.len(), MAX_DISCORD_USER_IDS);
        assert_eq!(ids[0], "00000000000000001");

        let with_junk = vec![
            DiscordMention {
                id: format!(" {} ", "1".repeat(17)),
                nickname: String::new(),
            },
            DiscordMention {
                id: "1".repeat(17),
                nickname: String::new(),
            }, // duplicate after trimming
            DiscordMention {
                id: "not-an-id".into(),
                nickname: String::new(),
            },
            DiscordMention {
                id: "2".repeat(20),
                nickname: String::new(),
            },
        ];
        assert_eq!(
            allowed_mention_ids(&with_junk),
            vec!["1".repeat(17), "2".repeat(20)]
        );
    }

    #[test]
    fn normalizes_theme_and_backdrop_with_fallbacks() {
        let fallback = AppSettings::default();
        let settings = normalize_settings(
            &json!({ "themeMode": "light", "windowBackdrop": "acrylic" }),
            &fallback,
        );
        assert_eq!(settings.theme_mode, ThemeMode::Light);
        assert_eq!(settings.window_backdrop, BackdropMode::Acrylic);

        let invalid = normalize_settings(
            &json!({ "themeMode": "blue", "windowBackdrop": 7 }),
            &fallback,
        );
        // An unreadable value takes the fallback, whatever the fallback happens to be.
        assert_eq!(invalid.theme_mode, fallback.theme_mode);
        assert_eq!(invalid.window_backdrop, fallback.window_backdrop);
    }

    #[test]
    fn merges_theme_and_backdrop_choices() {
        let current = AppSettings {
            theme_mode: ThemeMode::Light,
            window_backdrop: BackdropMode::Mica,
            ..AppSettings::default()
        };
        let merged = merge_settings(&current, &json!({ "windowBackdrop": "acrylic" }));
        assert_eq!(merged.theme_mode, ThemeMode::Light);
        assert_eq!(merged.window_backdrop, BackdropMode::Acrylic);
    }

    /// The template cap and the mention budget must account for each other exactly.
    ///
    /// This is the regression the fix exists for: the store used to cap templates at a flat 1,800
    /// while the editor validated the *rendered* message against Discord's 2,000, so a template
    /// between the two was accepted and then silently shortened. Anything that changes either
    /// number, or the mention format, has to change this derivation too.
    #[test]
    fn the_template_cap_leaves_exactly_the_worst_case_mention_budget() {
        // 5 ids of the longest Discord issues (20 digits), each rendered `<@…>` (23), single-spaced.
        assert_eq!(MAX_MENTION_EXPANSION_LENGTH, 5 * 23 + 4);
        assert_eq!(
            MAX_TEMPLATE_LENGTH + MAX_MENTION_EXPANSION_LENGTH,
            MAX_CONTENT_LENGTH
        );
        // Floors on the shipped values, not just on the relationship: a cap small enough to hurt
        // ordinary messages must not slip in behind a correct-looking budget sum. Compared through
        // locals so this stays a runtime assertion on the constant rather than a lintable
        // constant-expression.
        let cap = MAX_TEMPLATE_LENGTH;
        let content_limit = MAX_CONTENT_LENGTH;
        assert!(
            cap >= 1000,
            "a limit this small would be a usability regression: {cap}"
        );
        assert!(
            cap < content_limit,
            "the cap must stay below Discord's own limit to leave the mention budget room"
        );
    }

    /// A shipped default is a template like any other, so it has to fit the cap it is stored under.
    #[test]
    fn every_default_template_fits_the_cap() {
        let defaults = AppSettings::default();
        for template in [
            &defaults.notification_messages.queue_popped,
            &defaults.notification_messages.auto_accepted,
            &defaults.notification_messages.game_started,
        ] {
            assert!(
                template.chars().count() <= MAX_TEMPLATE_LENGTH,
                "{template:?} does not fit the cap"
            );
        }
    }

    /// The store keeps a template exactly as long as the editor will accept, and a longer one is
    /// still clamped (defence in depth for a hand-written file), which is why the editor must not
    /// accept more than [`MAX_TEMPLATE_LENGTH`].
    #[test]
    fn stores_a_template_up_to_the_cap_intact_and_clamps_a_longer_one() {
        let at_cap = "t".repeat(MAX_TEMPLATE_LENGTH);
        let stored = normalize_settings(
            &json!({ "notificationMessages": { "autoAccepted": at_cap } }),
            &AppSettings::default(),
        );
        assert_eq!(
            stored.notification_messages.auto_accepted.chars().count(),
            MAX_TEMPLATE_LENGTH,
            "a template the editor accepts must not be shortened"
        );

        let one_over = "t".repeat(MAX_TEMPLATE_LENGTH + 1);
        let clamped = normalize_settings(
            &json!({ "notificationMessages": { "autoAccepted": one_over } }),
            &AppSettings::default(),
        );
        assert_eq!(
            clamped.notification_messages.auto_accepted.chars().count(),
            MAX_TEMPLATE_LENGTH
        );
    }
}
