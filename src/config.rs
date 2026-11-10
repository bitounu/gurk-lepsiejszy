use anyhow::{Context, anyhow, bail};
use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, Visitor},
};
use tracing::warn;
use url::Url;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::{fmt, fs};

use crate::{command::ModeKeybindingConfig, passphrase::Passphrase};

const GURK_DB_NAME: &str = "gurk.sqlite";
const SIGNAL_DB_NAME: &str = "signal.sqlite";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Convert standalone ASCII emoticons in the composer, opt-in for old configs.
    #[serde(default)]
    pub auto_ascii_emoticons: bool,
    /// Offline dictionary spelling checks; message text never leaves this process.
    #[serde(default)]
    pub spellcheck: SpellcheckConfig,
    /// Strip descriptive JPEG/PNG metadata before uploading attachments.
    #[serde(default = "default_true")]
    pub clean_image_metadata: bool,
    /// Display messages by sender timestamp or stable local receipt order.
    #[serde(default)]
    pub message_order: MessageOrder,
    /// Directory to store messages and signal database, and attachments.
    #[serde(
        default = "default_data_dir",
        skip_serializing_if = "is_default_data_dir"
    )]
    pub data_dir: PathBuf,
    /// Path to the Signal database containing the linked device data.
    #[serde(
        rename = "signal_db_path",
        default = "default_signal_db_path",
        skip_serializing_if = "is_default_signal_db_path"
    )]
    pub deprecated_signal_db_path: PathBuf,
    /// Whether only to show the first name of a contact
    #[serde(default)]
    pub first_name_only: bool,
    /// Show inline image attachment previews (Unicode half blocks).
    #[serde(default)]
    pub show_image_previews: bool,
    /// Show full attachment file URLs instead of filenames in messages.
    #[serde(default)]
    pub show_attachment_paths: bool,
    /// Fullscreen and external image viewer settings.
    #[serde(default)]
    pub image_viewer: ImageViewerConfig,
    /// Optional context-sensitive shortcut row at the bottom of the terminal.
    #[serde(default)]
    pub shortcut_bar: ShortcutBarConfig,
    /// File type glyphs require a Nerd Font; false uses text labels instead.
    #[serde(default = "default_true")]
    pub nerd_font_icons: bool,
    /// Screenshot directory, defaulting to ~/Pictures if present, otherwise HOME.
    #[serde(default)]
    pub screenshot_dir: Option<PathBuf>,
    /// Optional screenshot program and arguments; output path is appended as one argument.
    #[serde(default)]
    pub screenshot_command: Vec<String>,
    /// Whether to show receipts (sent, delivered, read) information next to your user name in UI
    #[serde(default = "default_true")]
    pub show_receipts: bool,
    /// Notification settings
    #[serde(default, deserialize_with = "deserialize_notification_config")]
    pub notifications: NotificationConfig,
    #[serde(default = "default_true")]
    pub bell: bool,
    /// User configuration
    pub user: User,
    #[cfg(feature = "dev")]
    #[serde(default, skip_serializing_if = "DeveloperConfig::is_default")]
    pub developer: DeveloperConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sqlite: Option<SqliteConfig>,
    #[serde(default)]
    /// If set, enables encryption of the key store and messages database
    pub passphrase: Option<Passphrase>,
    /// If set, the full message text will be colored, not only the author name
    #[serde(default)]
    pub colored_messages: bool,
    #[serde(default)]
    /// Keymaps
    pub keybindings: ModeKeybindingConfig,
    /// Whether to enable the default keybindings
    #[serde(default = "default_true")]
    pub default_keybindings: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpellcheckConfig {
    pub enabled: bool,
    pub languages: Vec<String>,
    pub dictionary_dirs: Vec<PathBuf>,
    pub ignored_words: Vec<String>,
}

impl Default for SpellcheckConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            languages: vec!["pl_PL".into(), "en_US".into()],
            dictionary_dirs: Vec::new(),
            ignored_words: Vec::new(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrder {
    /// Preserve the historical sender timestamp ordering.
    #[default]
    Sent,
    /// Display new messages in the order first stored locally.
    Received,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutBarTheme {
    #[default]
    Contrast,
    Dark,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutBarConfig {
    pub enabled: bool,
    pub theme: ShortcutBarTheme,
    #[serde(
        deserialize_with = "deserialize_shortcut_color",
        skip_serializing_if = "Option::is_none"
    )]
    pub foreground: Option<String>,
    #[serde(
        deserialize_with = "deserialize_shortcut_color",
        skip_serializing_if = "Option::is_none"
    )]
    pub background: Option<String>,
}

fn deserialize_shortcut_color<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let color = Option::<String>::deserialize(deserializer)?;
    if color.as_ref().is_some_and(|color| {
        color.len() != 7
            || !color.starts_with('#')
            || !color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
    }) {
        return Err(de::Error::custom("shortcut bar color must be #RRGGBB"));
    }
    Ok(color)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageViewerMode {
    #[default]
    Internal,
    External,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageViewerProtocol {
    #[default]
    Auto,
    Halfblocks,
    Kitty,
    Sixel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageViewerConfig {
    pub mode: ImageViewerMode,
    /// Program and arguments; the image path is appended without a shell.
    pub command: Vec<String>,
    pub protocol: ImageViewerProtocol,
}

impl Default for ImageViewerConfig {
    fn default() -> Self {
        Self {
            mode: ImageViewerMode::Internal,
            command: vec!["sxiv".into(), "-a".into()],
            protocol: ImageViewerProtocol::Auto,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    /// Name to be shown in the application
    #[serde(alias = "name")]
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationConfig {
    /// Whether to show system notifications on incoming messages
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Whether to show message preview in notifications
    #[serde(default = "default_true")]
    pub show_message_text: bool,
    /// Whether to show message origin in notifications
    #[serde(default = "default_true")]
    pub show_message_chat: bool,
    /// Whether to show reactions in notifications
    #[serde(default = "default_true")]
    pub show_reactions: bool,
    /// Whether to mute reactions bell
    #[serde(default)]
    pub mute_reactions_bell: bool,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            show_message_text: true,
            show_message_chat: true,
            show_reactions: true,
            mute_reactions_bell: false,
        }
    }
}

#[cfg(feature = "dev")]
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeveloperConfig {
    /// Dump raw messages to `messages.json` for collecting debug/benchmark data
    pub dump_raw_messages: bool,
}

#[cfg(feature = "dev")]
impl DeveloperConfig {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub(crate) config: Config,
    pub(crate) deprecated_keys: DeprecatedKeys,
}

impl LoadedConfig {
    pub fn report_deprecated_keys(self) -> Config {
        if !self.deprecated_keys.keys.is_empty() {
            println!("In '{}':", self.deprecated_keys.file_path.display());
            for DeprecatedConfigKey { key, message } in self.deprecated_keys.keys.iter() {
                warn!(key, message, "deprecated config key");
                println!("deprecated config key: {key}, {message}");
            }
        }
        self.config
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DeprecatedKeys {
    pub(crate) file_path: PathBuf,
    pub(crate) keys: Vec<DeprecatedConfigKey>,
}

#[derive(Debug, Clone)]
pub(crate) struct DeprecatedConfigKey {
    pub(crate) key: &'static str,
    pub(crate) message: &'static str,
}

/// Accepts either `notifications = true/false` (legacy) or `[notifications]` (current struct).
fn deserialize_notification_config<'de, D>(deserializer: D) -> Result<NotificationConfig, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct NotificationConfigVisitor;

    impl<'de> Visitor<'de> for NotificationConfigVisitor {
        type Value = NotificationConfig;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a boolean or a notification config table")
        }

        fn visit_bool<E: de::Error>(self, enabled: bool) -> Result<NotificationConfig, E> {
            Ok(NotificationConfig {
                enabled,
                ..Default::default()
            })
        }

        fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<NotificationConfig, M::Error> {
            NotificationConfig::deserialize(de::value::MapAccessDeserializer::new(map))
        }
    }

    deserializer.deserialize_any(NotificationConfigVisitor)
}

/// Writes `content` to `path` with owner-only permissions (0o600 on Unix).
///
/// This is important in case the passphrase is stored in the config file.
fn write_config(path: &Path, content: &str) -> anyhow::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(content.as_bytes())?;
    Ok(())
}

impl Config {
    /// Create new config with default paths from the given user.
    pub fn with_user(user: User) -> Self {
        Config {
            auto_ascii_emoticons: false,
            message_order: MessageOrder::default(),
            user,
            spellcheck: SpellcheckConfig::default(),
            clean_image_metadata: true,
            data_dir: default_data_dir(),
            deprecated_signal_db_path: default_signal_db_path(),
            first_name_only: false,
            show_image_previews: false,
            show_attachment_paths: false,
            image_viewer: ImageViewerConfig::default(),
            shortcut_bar: ShortcutBarConfig::default(),
            nerd_font_icons: true,
            screenshot_dir: None,
            screenshot_command: Vec::new(),
            show_receipts: true,
            notifications: NotificationConfig::default(),
            bell: true,
            #[cfg(feature = "dev")]
            developer: Default::default(),
            sqlite: Default::default(),
            passphrase: None,
            colored_messages: false,
            default_keybindings: true,
            keybindings: ModeKeybindingConfig::default(),
        }
    }

    /// Tries to load configuration from one of the default locations:
    ///
    /// 1. $XDG_CONFIG_HOME/gurk/gurk.toml
    /// 2. $XDG_CONFIG_HOME/gurk.toml
    /// 3. $HOME/.config/gurk/gurk.toml
    /// 4. $HOME/.gurk.toml
    ///
    /// If no config is found returns `None`.
    pub fn load_installed() -> anyhow::Result<Option<LoadedConfig>> {
        installed_config().map(Self::load).transpose()
    }

    pub fn load_installed_passphrase() -> anyhow::Result<Option<Passphrase>> {
        let loaded = Self::load_installed()?;
        Ok(loaded.and_then(|c| c.config.passphrase))
    }

    /// Saves a new config file in case it does not exist.
    ///
    /// Also makes sure that the `config.data_path` exists.
    pub fn save_new(&self) -> anyhow::Result<PathBuf> {
        // Prefer XDG_CONFIG_HOME if set, otherwise use platform default
        let config_dir = if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(xdg_config)
        } else {
            dirs::config_dir().ok_or_else(|| anyhow!("could not find default config directory"))?
        };
        let config_file = config_dir.join("gurk/gurk.toml");
        self.save_new_at(&config_file)
            .with_context(|| format!("failed to save config at {}", config_file.display()))?;
        Ok(config_file)
    }

    fn save_new_at(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        // check that config won't be overridden
        if path.as_ref().exists() {
            bail!(
                "will not override config file at: {}",
                path.as_ref().display()
            );
        }

        // make sure data_path exists
        let data_path = default_data_dir();
        fs::create_dir_all(data_path).context("could not create data dir")?;

        self.save(path)
    }

    /// Load config from a specific path.
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<LoadedConfig> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path)?;
        let config = toml::de::from_str(&content)?;

        // check for deprecated keys
        let config_value: toml::Value = toml::de::from_str(&content)?;
        let mut keys = Vec::new();
        if config_value
            .get("sqlite")
            .map(|v| v.get("enabled").is_some())
            .unwrap_or(false)
        {
            keys.push(DeprecatedConfigKey {
                key: "sqlite.enabled",
                message: "sqlite is now enabled by default",
            });
        }
        if config_value.get("signal_db_path").is_some() {
            keys.push(DeprecatedConfigKey {
                key: "signal_db_path",
                message: "is not used anymore; use `data_dir` instead",
            });
        }
        if config_value.get("sqlite").is_some() {
            keys.push(DeprecatedConfigKey {
                key: "sqlite",
                message: "will be removed in a future version; use `<data_dir>/gurk.sqlite` instead",
            });
        }
        if config_value
            .get("notifications")
            .and_then(|v| v.as_bool())
            .is_some()
        {
            keys.push(DeprecatedConfigKey {
                key: "notifications",
                message: "boolean format is deprecated; use [notifications] section with enabled, show_message_text, show_message_chat, show_reactions and mute_reactions_bell fields",
            });
        }

        let deprecated_keys = DeprecatedKeys {
            file_path: path.to_path_buf(),
            keys,
        };

        Ok(LoadedConfig {
            config,
            deprecated_keys,
        })
    }

    fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path = path.as_ref();
        let content = toml::ser::to_string(self)?;
        let parent_dir = path
            .parent()
            .ok_or_else(|| anyhow!("invalid config path {}: no parent dir", path.display()))?;
        fs::create_dir_all(parent_dir).unwrap();
        write_config(path, &content)?;
        Ok(())
    }

    pub fn gurk_db_path(&self) -> PathBuf {
        self.data_dir.join(GURK_DB_NAME)
    }

    pub(crate) fn signal_db_path(&self) -> PathBuf {
        self.data_dir.join(SIGNAL_DB_NAME)
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub struct SqliteConfig {
    #[serde(default = "SqliteConfig::default_db_url")]
    pub url: Url,
    /// Don't delete the unencrypted db, after applying encryption to it
    ///
    /// Useful for testing.
    #[serde(default, rename = "_preserve_unencrypted")]
    pub preserve_unencrypted: bool,
}

impl SqliteConfig {
    fn default_db_url() -> Url {
        let path = default_data_dir().join("gurk.sqlite");
        format!("sqlite://{}", path.display())
            .parse()
            .expect("invalid default sqlite path")
    }
}

/// Get the location of the first found default config file paths
/// according to the following order:
///
/// 1. $XDG_CONFIG_HOME/gurk/gurk.toml (if XDG_CONFIG_HOME is set)
/// 2. $XDG_CONFIG_HOME/gurk.toml (if XDG_CONFIG_HOME is set)
/// 3. Platform config dir (e.g. ~/Library/Application Support on macOS)/gurk/gurk.toml
/// 4. Platform config dir/gurk.toml
/// 5. $HOME/.gurk.toml
fn installed_config() -> Option<PathBuf> {
    // Check XDG_CONFIG_HOME first (works on all platforms if set)
    if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
        let xdg_dir = PathBuf::from(xdg_config);
        let config_file = xdg_dir.join("gurk/gurk.toml");
        if config_file.exists() {
            return Some(config_file);
        }
        let config_file = xdg_dir.join("gurk.toml");
        if config_file.exists() {
            return Some(config_file);
        }
    }

    // Fall back to platform-specific config dir
    let config_dir = dirs::config_dir()?;
    let config_file = config_dir.join("gurk/gurk.toml");
    if config_file.exists() {
        return Some(config_file);
    }

    let config_file = config_dir.join("gurk.toml");
    if config_file.exists() {
        return Some(config_file);
    }

    // Legacy: ~/.gurk.toml
    let home_dir = dirs::home_dir()?;
    let config_file = home_dir.join(".gurk.toml");
    if config_file.exists() {
        return Some(config_file);
    }

    None
}

/// Path to store the signal database containing the data for the linked device.
fn default_signal_db_path() -> PathBuf {
    default_data_dir().join("signal-db")
}

fn is_default_signal_db_path(path: &Path) -> bool {
    path == default_signal_db_path()
}

/// Fallback to legacy data path location
pub fn fallback_data_path() -> Option<PathBuf> {
    dirs::home_dir().map(|p| p.join(".gurk.data.json"))
}

fn default_data_dir() -> PathBuf {
    // Check XDG_DATA_HOME first (works on all platforms if set)
    if let Ok(xdg_data) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(xdg_data).join("gurk");
    }
    // Fall back to platform-specific data dir
    let data_dir =
        dirs::data_dir().expect("data directory not found, $XDG_DATA_HOME and $HOME are unset?");
    data_dir.join("gurk")
}

fn is_default_data_dir(path: &Path) -> bool {
    path == default_data_dir()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{NamedTempFile, TempDir, tempdir};

    fn example_config_with_random_paths(dir: &TempDir) -> Config {
        let data_dir = dir.path().join("some-data-dir/some-other-dir/data.json");
        assert!(!data_dir.parent().unwrap().exists());

        Config {
            data_dir,
            ..Config::with_user(User {
                display_name: "Tyler Durden".to_string(),
            })
        }
    }

    #[test]
    fn message_order_defaults_and_explicit_modes() {
        let legacy: Config = toml::from_str("[user]\nname = \"Legacy\"\n").unwrap();
        assert_eq!(legacy.message_order, MessageOrder::Sent);
        assert_eq!(
            Config::with_user(legacy.user).message_order,
            MessageOrder::Sent
        );
        for (value, expected) in [
            ("sent", MessageOrder::Sent),
            ("received", MessageOrder::Received),
        ] {
            let text = format!("message_order = {value:?}\n[user]\nname = \"Test\"\n");
            let config: Config = toml::from_str(&text).unwrap();
            assert_eq!(config.message_order, expected);
            assert_eq!(
                toml::from_str::<Config>(&toml::to_string(&config).unwrap()).unwrap(),
                config
            );
        }
        assert!(
            toml::from_str::<Config>("message_order = \"unknown\"\n[user]\nname = \"Test\"\n")
                .is_err()
        );
    }

    #[test]
    fn shortcut_bar_defaults_and_colors() {
        let legacy: Config = toml::from_str("[user]\nname = \"Legacy\"\n").unwrap();
        assert_eq!(legacy.shortcut_bar, ShortcutBarConfig::default());
        assert!(!legacy.shortcut_bar.enabled);
        assert_eq!(legacy.shortcut_bar.theme, ShortcutBarTheme::Contrast);
        assert_eq!(
            Config::with_user(legacy.user).shortcut_bar,
            ShortcutBarConfig::default()
        );
        let configured: ShortcutBarConfig = toml::from_str(
            "enabled = true\ntheme = \"dark\"\nforeground = \"#aBcD01\"\nbackground = \"#000000\"",
        )
        .unwrap();
        assert!(configured.enabled);
        assert_eq!(configured.theme, ShortcutBarTheme::Dark);
        assert_eq!(configured.foreground.as_deref(), Some("#aBcD01"));
        assert_eq!(
            toml::from_str::<ShortcutBarConfig>(&toml::to_string(&configured).unwrap()).unwrap(),
            configured
        );
        for color in ["red", "123456", "#12345", "#1234567", "#gg0000", "#é0000"] {
            for field in ["foreground", "background"] {
                let source =
                    format!("[user]\nname = \"Test\"\n[shortcut_bar]\n{field} = {color:?}");
                assert!(
                    toml::from_str::<Config>(&source).is_err(),
                    "{field}: {color}"
                );
            }
        }
    }

    #[test]
    fn test_save_new_at_non_existent() -> anyhow::Result<()> {
        let dir = tempdir()?;

        let config = example_config_with_random_paths(&dir);
        let config_path = dir.path().join("some-dir/some-other-dir/gurk.toml");

        config.save_new_at(&config_path)?;
        let LoadedConfig {
            config: loaded_config,
            deprecated_keys: _,
        } = Config::load(&config_path)?;
        assert_eq!(config, loaded_config);

        assert!(config_path.parent().unwrap().exists()); // data path parent is created

        Ok(())
    }

    #[test]
    fn composer_options_accept_old_configs_and_custom_screenshot_paths() {
        let old: Config = toml::from_str("[user]\nname = \"Old\"\n").unwrap();
        assert!(old.screenshot_dir.is_none());
        assert!(old.screenshot_command.is_empty());
        let configured: Config = toml::from_str("screenshot_dir = \"~/Shots\"\nnerd_font_icons = false\nscreenshot_command = [\"maim\", \"-s\"]\n[user]\ndisplay_name = \"Test\"\n").unwrap();
        assert_eq!(
            configured.screenshot_dir.as_deref(),
            Some(Path::new("~/Shots"))
        );
        assert!(!configured.nerd_font_icons);
        assert_eq!(configured.screenshot_command, ["maim", "-s"]);
    }

    #[test]
    fn image_viewer_legacy_defaults_and_explicit_configuration() {
        let legacy: Config = toml::from_str("[user]\nname = \"Legacy\"\n").unwrap();
        assert_eq!(legacy.image_viewer.mode, ImageViewerMode::Internal);
        assert_eq!(legacy.image_viewer.protocol, ImageViewerProtocol::Auto);
        let explicit: Config = toml::from_str(
            r#"
[user]
display_name = "Test"
[image_viewer]
mode = "external"
command = ["sxiv", "-a"]
protocol = "halfblocks"
"#,
        )
        .unwrap();
        assert_eq!(explicit.image_viewer.mode, ImageViewerMode::External);
        assert_eq!(explicit.image_viewer.command, ["sxiv", "-a"]);
        assert_eq!(
            explicit.image_viewer.protocol,
            ImageViewerProtocol::Halfblocks
        );
        let serialized = toml::to_string(&explicit).unwrap();
        assert_eq!(toml::from_str::<Config>(&serialized).unwrap(), explicit);
    }

    #[test]
    fn ascii_emoticons_config_is_opt_in_and_roundtrips() {
        let old: Config = toml::from_str("[user]\ndisplay_name = 'Test'\n").unwrap();
        assert!(!old.auto_ascii_emoticons);
        assert!(!Config::with_user(old.user.clone()).auto_ascii_emoticons);
        let enabled: Config =
            toml::from_str("auto_ascii_emoticons = true\n[user]\ndisplay_name = 'Test'\n").unwrap();
        assert!(enabled.auto_ascii_emoticons);
        let text = toml::to_string(&enabled).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), enabled);
    }

    #[test]
    fn image_previews_are_opt_in_and_old_config_loads() {
        let legacy: Config = toml::from_str("[user]\nname = \"Legacy\"\n").unwrap();
        assert!(!legacy.show_image_previews);
        let enabled: Config =
            toml::from_str("show_image_previews = true\n[user]\ndisplay_name = \"Test\"\n")
                .unwrap();
        assert!(enabled.show_image_previews);
    }

    #[test]
    fn attachment_paths_are_opt_in() {
        let legacy: Config = toml::from_str("[user]\nname = \"Legacy\"\n").unwrap();
        assert!(!legacy.show_attachment_paths);
        assert!(!Config::with_user(legacy.user.clone()).show_attachment_paths);
        let enabled: Config =
            toml::from_str("show_attachment_paths = true\n[user]\nname = \"Test\"\n").unwrap();
        assert!(enabled.show_attachment_paths);
        let serialized = toml::to_string(&enabled).unwrap();
        assert_eq!(toml::from_str::<Config>(&serialized).unwrap(), enabled);
    }

    #[test]
    fn test_notifications_bool_compat() {
        // Old configs with `notifications = true/false` should parse successfully
        let toml_true = r#"
notifications = true
[user]
display_name = "Test"
"#;
        let config: Config = toml::de::from_str(toml_true).unwrap();
        assert!(config.notifications.enabled);

        let toml_false = r#"
notifications = false
[user]
display_name = "Test"
"#;
        let config: Config = toml::de::from_str(toml_false).unwrap();
        assert!(!config.notifications.enabled);

        // New struct format should still work
        let toml_struct = r#"
[user]
display_name = "Test"
[notifications]
enabled = false
show_message_text = true
"#;
        let config: Config = toml::de::from_str(toml_struct).unwrap();
        assert!(!config.notifications.enabled);
        assert!(config.notifications.show_message_text);
    }

    #[test]
    fn test_save_new_fails_or_existent() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let config = example_config_with_random_paths(&dir);
        let file = NamedTempFile::new()?;

        assert!(config.save_new_at(file.path()).is_err());

        Ok(())
    }
}
