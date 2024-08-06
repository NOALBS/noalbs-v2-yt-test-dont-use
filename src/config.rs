use std::{collections::HashMap, io::Seek};

use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::{chat, error, stream_servers, switcher};

const MAX_LOW_RETRY: u8 = 5;

/// The config of NOALBS
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub user: User,
    pub switcher: Switcher,
    pub software: SoftwareConnection,
    pub chat: Option<Chat>,
    pub optional_scenes: OptionalScenes,
    pub optional_options: OptionalOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: Option<i64>,
    pub name: String,
    pub password_hash: Option<String>,
}

/// All the data that can be changed outside of the switcher
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Switcher {
    /// Disable the switcher
    pub bitrate_switcher_enabled: bool,

    /// Only enable the switcher when actually streaming from OBS
    pub only_switch_when_streaming: bool,

    /// When stream comes back from offline, instantly switch to low / live
    pub instantly_switch_on_recover: bool,

    /// Enable auto switch chat notification
    pub auto_switch_notification: bool,

    /// Max attempts to poll the bitrate every second on low bitrate / offline.
    /// This will be used to make sure the stream is actually in a low / offline
    /// bitrate state
    pub retry_attempts: u8,

    /// Triggers to switch to the low or offline scenes
    pub triggers: switcher::Triggers,

    /// The default switching scenes
    pub switching_scenes: switcher::SwitchingScenes,

    /// Add multiple stream servers to watch before switching to low or offline
    pub stream_servers: Vec<stream_servers::StreamServer>,
}

impl Switcher {
    pub fn add_stream_server(&mut self, stream_server: stream_servers::StreamServer) {
        self.stream_servers.push(stream_server);

        self.sort_stream_servers();
    }

    /// Sort by highest number first
    pub fn sort_stream_servers(&mut self) {
        self.stream_servers
            .sort_by(|a, b| a.priority.cmp(&b.priority));
    }

    pub fn set_bitrate_switcher_enabled(&mut self, enabled: bool) {
        self.bitrate_switcher_enabled = enabled;

        //if enabled {
        //    self.switcher_enabled_notifier.notify_waiters();
        //}
    }
}

impl Default for Switcher {
    fn default() -> Self {
        Self {
            bitrate_switcher_enabled: true,
            only_switch_when_streaming: true,
            instantly_switch_on_recover: true,
            auto_switch_notification: true,
            triggers: switcher::Triggers::default(),
            stream_servers: Vec::new(),
            switching_scenes: switcher::SwitchingScenes {
                normal: "live".to_string(),
                low: "low".to_string(),
                offline: "offline".to_string(),
            },
            retry_attempts: MAX_LOW_RETRY,
        }
    }
}

// TODO: Is it possible to do this another way?
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum SoftwareConnection {
    ObsOld(ObsConfig),
    Obs(ObsConfig),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ObsConfig {
    pub host: String,
    pub password: Option<String>,
    pub port: u16,

    // Configurable profile and collection pairs
    pub collections: Option<HashMap<String, CollectionPair>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CollectionPair {
    /// OBS profile name
    pub profile: String,

    /// OBS collection name
    pub collection: String,
}

pub trait ConfigLogic: Send + Sync {
    fn load(&self) -> Result<Config, error::Error>;
    fn save(&self, config: &Config) -> Result<(), error::Error>;
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Chat {
    pub platform: ConfigChatPlatform,
    pub username: String,
    pub admins: Vec<String>,
    pub language: chat::ChatLanguage,

    pub prefix: String,

    pub enable_public_commands: bool,
    pub enable_mod_commands: bool,
    pub enable_auto_stop_stream_on_host_or_raid: bool,
    pub announce_raid_on_auto_stop: bool,
    pub commands: Option<HashMap<chat::Command, CommandInfo>>,
}

impl Default for Chat {
    fn default() -> Self {
        Self {
            platform: ConfigChatPlatform::Twitch,
            username: "715209".to_string(),
            admins: vec![],
            language: chat::ChatLanguage::EN,
            prefix: "!".to_string(),
            enable_public_commands: true,
            enable_mod_commands: true,
            enable_auto_stop_stream_on_host_or_raid: true,
            announce_raid_on_auto_stop: true,
            commands: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigChatPlatform {
    Twitch,
    Kick(KickConfig),
    Youtube,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "camelCase")]
pub struct KickConfig {
    pub channel_id: Option<usize>,
    pub chatroom_id: Option<usize>,
    pub use_irlproxy: Option<bool>,
}

impl ConfigChatPlatform {
    pub fn kind(&self) -> chat::ChatPlatform {
        match self {
            ConfigChatPlatform::Twitch => chat::ChatPlatform::Twitch,
            ConfigChatPlatform::Kick(_) => chat::ChatPlatform::Kick,
            ConfigChatPlatform::Youtube => chat::ChatPlatform::Youtube,
        }
    }
}


#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CommandInfo {
    pub permission: Option<chat::Permission>,
    pub user_permissions: Option<Vec<String>>,
    pub alias: Option<Vec<String>>,
}

pub struct File {
    pub name: std::path::PathBuf,
}

impl ConfigLogic for File {
    fn load(&self) -> Result<Config, error::Error> {
        let mut file = std::fs::File::open(&self.name).map_err(error::Error::ConfigFileError)?;
        let mut config: Config = match serde_json::from_reader(&file) {
            Ok(c) => c,
            Err(e) => {
                // Check if config v1
                file.rewind()?;
                let old: serde_json::Result<ConfigOld> = serde_json::from_reader(&file);

                if let Ok(o) = old {
                    info!("Converting old NOALBS config into v2");

                    if std::fs::File::open(".env").is_err() {
                        info!("Creating .env file");

                        let bot = o.twitch_chat.bot_username.to_lowercase();
                        let oauth = o.twitch_chat.oauth.to_string();
                        let env =
                            format!("TWITCH_BOT_USERNAME={}\nTWITCH_BOT_OAUTH={}", bot, oauth);
                        std::fs::write(".env", env.as_bytes())?;

                        std::env::set_var("TWITCH_BOT_USERNAME", bot);
                        std::env::set_var("TWITCH_BOT_OAUTH", oauth);
                    }

                    let c = Config::from(o);
                    self.save(&c)?;

                    c
                } else {
                    return Err(error::Error::Json(e));
                }
            }
        };

        config.switcher.sort_stream_servers();

        if let Some(chat) = &mut config.chat {
            chat.username.make_ascii_lowercase();

            for admin in &mut chat.admins {
                admin.make_ascii_lowercase();
            }

            if let Some(commands) = &mut chat.commands {
                commands
                    .values_mut()
                    .filter_map(|c| c.user_permissions.as_mut())
                    .flatten()
                    .for_each(|u| u.make_ascii_lowercase());
            }
        }

        Ok(config)
    }

    // TODO: Handle error
    fn save(&self, config: &Config) -> Result<(), error::Error> {
        let file = std::fs::File::create(&self.name)?;
        serde_json::to_writer_pretty(file, config).unwrap();

        Ok(())
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct OptionalScenes {
    pub starting: Option<String>,
    pub ending: Option<String>,
    pub privacy: Option<String>,
    pub refresh: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OptionalOptions {
    pub twitch_transcoding_check: bool,
    pub twitch_transcoding_retries: u64,
    pub twitch_transcoding_delay_seconds: u64,

    /// Automatically stop the stream after n minutes on the offline scene
    pub offline_timeout: Option<u32>,

    /// Automatically start recording while streaming
    pub record_while_streaming: bool,

    /// Switch to the starting scene when the stream starts
    pub switch_to_starting_scene_on_stream_start: bool,

    /// Switch from the starting scene to the live scene when the stream starts
    pub switch_from_starting_scene_to_live_scene: bool,
}

impl Default for OptionalOptions {
    fn default() -> Self {
        Self {
            twitch_transcoding_check: false,
            twitch_transcoding_retries: 5,
            twitch_transcoding_delay_seconds: 15,
            offline_timeout: None,
            record_while_streaming: false,
            switch_to_starting_scene_on_stream_start: false,
            switch_from_starting_scene_to_live_scene: false,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigOld {
    obs: ObsOld,
    rtmp: RtmpOld,
    twitch_chat: TwitchChat,
    language: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ObsOld {
    ip: String,
    password: String,
    normal_scene: String,
    offline_scene: String,
    low_bitrate_scene: String,
    refresh_scene: String,
    low_bitrate_trigger: u32,
    high_rtt_trigger: Option<u32>,
    refresh_scene_interval: u32,
    only_switch_when_streaming: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RtmpOld {
    server: String,
    stats: String,
    application: Option<String>,
    key: Option<String>,
    id: Option<String>,
    publisher: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TwitchChat {
    channel: String,
    bot_username: String,
    oauth: String,
    enable: bool,
    prefix: String,
    enable_public_commands: bool,
    public_commands: Vec<String>,
    enable_mod_commands: bool,
    mod_commands: Vec<String>,
    enable_auto_switch_notification: bool,
    enable_auto_stop_stream_on_host_or_raid: bool,
    admin_users: Vec<String>,
    alias: Option<Vec<Vec<String>>>,
}

impl From<ConfigOld> for Config {
    fn from(o: ConfigOld) -> Self {
        let mut full_host = o.obs.ip.split(':');
        let software = SoftwareConnection::Obs(ObsConfig {
            host: full_host.next().unwrap().to_owned(),
            password: Some(o.obs.password),
            port: full_host.next().unwrap().parse().unwrap(),
            collections: Some(HashMap::new()),
        });

        let mut config = Config {
            user: User {
                id: None,
                name: o.twitch_chat.channel.to_owned(),
                password_hash: None,
            },
            switcher: Switcher {
                only_switch_when_streaming: o.obs.only_switch_when_streaming,
                auto_switch_notification: o.twitch_chat.enable_auto_switch_notification,
                triggers: switcher::Triggers {
                    low: Some(o.obs.low_bitrate_trigger),
                    rtt: o.obs.high_rtt_trigger,
                    offline: None,
                    rtt_offline: None,
                },
                switching_scenes: switcher::SwitchingScenes {
                    normal: o.obs.normal_scene,
                    low: o.obs.low_bitrate_scene,
                    offline: o.obs.offline_scene,
                },
                ..Default::default()
            },
            software,
            chat: Some(Chat {
                platform: ConfigChatPlatform::Twitch,
                username: o.twitch_chat.channel,
                admins: o.twitch_chat.admin_users,
                prefix: o.twitch_chat.prefix,
                enable_auto_stop_stream_on_host_or_raid: o
                    .twitch_chat
                    .enable_auto_stop_stream_on_host_or_raid,
                commands: Some(HashMap::new()),
                enable_public_commands: o.twitch_chat.enable_public_commands,
                enable_mod_commands: o.twitch_chat.enable_mod_commands,
                ..Default::default()
            }),
            optional_scenes: OptionalScenes::default(),
            optional_options: OptionalOptions::default(),
        };

        let commands = config.chat.as_mut().unwrap().commands.as_mut().unwrap();

        for c in o.twitch_chat.mod_commands {
            update_command(commands, c, Some(chat::Permission::Mod), None)
        }

        for c in o.twitch_chat.public_commands {
            update_command(commands, c, Some(chat::Permission::Public), None)
        }

        if let Some(alias) = o.twitch_chat.alias {
            for c in alias {
                update_command(commands, c[1].clone(), None, Some(c[0].clone()))
            }
        }

        if !commands.contains_key(&chat::Command::Switch) {
            update_command(
                commands,
                "switch".to_string(),
                Some(chat::Permission::Mod),
                Some("ss".to_string()),
            );
        }

        if !commands.contains_key(&chat::Command::Fix) {
            update_command(
                commands,
                "fix".to_string(),
                Some(chat::Permission::Mod),
                Some("f".to_string()),
            );
        }

        let ss = stream_servers::StreamServer::from(o.rtmp);
        config.switcher.stream_servers.push(ss);

        if let Some(lang) = o.language {
            if let Ok(l) = lang.parse() {
                config.chat.as_mut().unwrap().language = l;
            }
        }

        config
    }
}

impl From<RtmpOld> for stream_servers::StreamServer {
    fn from(r: RtmpOld) -> Self {
        let mut name = if r.server == "nginx" || r.server == "node-media-server" {
            "RTMP"
        } else {
            "SRT"
        }
        .to_string();

        let stream_server: Box<dyn stream_servers::Bsl> = match r.server.as_ref() {
            "nginx" => Box::new(stream_servers::nginx::Nginx {
                stats_url: r.stats,
                application: r.application.unwrap(),
                key: r.key.unwrap(),
                client: reqwest::Client::new(),
            }),
            "node-media-server" => Box::new(stream_servers::nms::NodeMediaServer {
                stats_url: r.stats,
                application: r.application.unwrap(),
                key: r.key.unwrap(),
                auth: None,
                client: reqwest::Client::new(),
            }),
            "nimble" => Box::new(stream_servers::nimble::Nimble {
                id: r.id.unwrap(),
                stats_url: r.stats,
                application: r.application.unwrap(),
                key: r.key.unwrap(),
                client: reqwest::Client::new(),
            }),
            "srt-live-server" => {
                let stats_url = r.stats;
                let publisher = r.publisher.unwrap();

                if stats_url.contains("belabox.net") {
                    name = "BELABOX".to_string();
                    Box::new(stream_servers::belabox::Belabox {
                        stats_url,
                        publisher,
                        client: reqwest::Client::new(),
                    })
                } else {
                    Box::new(stream_servers::sls::SrtLiveServer {
                        stats_url,
                        publisher,
                        client: reqwest::Client::new(),
                    })
                }
            }
            _ => panic!("No supported server found"),
        };

        Self {
            stream_server,
            name,
            priority: Some(0),
            override_scenes: None,
            depends_on: None,
            enabled: true,
        }
    }
}

impl Default for ObsOld {
    fn default() -> Self {
        Self {
            ip: "localhost".to_string(),
            password: "password".to_string(),
            normal_scene: "live".to_string(),
            low_bitrate_scene: "low".to_string(),
            offline_scene: "offline".to_string(),
            refresh_scene: "refresh".to_string(),
            low_bitrate_trigger: 800,
            high_rtt_trigger: Some(2500),
            refresh_scene_interval: 10,
            only_switch_when_streaming: true,
        }
    }
}

impl Default for TwitchChat {
    fn default() -> Self {
        Self {
            channel: "715209".to_string(),
            bot_username: "715209".to_string(),
            oauth: "oauth:YOUR_OAUTH_HERE".to_string(),
            enable: true,
            prefix: "!".to_string(),
            enable_public_commands: true,
            public_commands: vec!["bitrate".to_string()],
            enable_mod_commands: true,
            mod_commands: vec![
                "refresh".to_string(),
                "fix".to_string(),
                "trigger".to_string(),
                "sourceinfo".to_string(),
                "obsinfo".to_string(),
            ],
            enable_auto_switch_notification: true,
            enable_auto_stop_stream_on_host_or_raid: true,
            admin_users: vec!["715209".to_string(), "b3ck".to_string()],
            alias: Some(vec![
                vec!["r".to_string(), "refresh".to_string()],
                vec!["f".to_string(), "fix".to_string()],
                vec!["b".to_string(), "bitrate".to_string()],
            ]),
        }
    }
}

fn update_command(
    config: &mut HashMap<chat::Command, CommandInfo>,
    command: String,
    permission: Option<chat::Permission>,
    alias: Option<String>,
) {
    let command = chat::Command::from(command.as_ref());

    if let chat::Command::Unknown(e) = command {
        error!("Found unrecognized command {}", e);
        return;
    }

    let c = config.entry(command).or_default();

    if permission.is_some() {
        c.permission = permission;
    }

    if let Some(alias) = alias {
        if c.alias.is_none() {
            c.alias = Some(Vec::new());
        }

        c.alias.as_mut().unwrap().push(alias);
    }
}
