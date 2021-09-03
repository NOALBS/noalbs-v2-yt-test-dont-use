use std::{collections::HashMap, sync::Arc};

use tokio::sync::RwLock;
use tracing::info;

use crate::{
    broadcasting_software::obs::Obs,
    chat, config, error,
    state::{self, State},
    stream_servers,
    switcher::{self, Switcher},
};

/// The state of the current user
pub type UserState = Arc<RwLock<State>>;

/// MPSC to send messages to chat
pub type ChatSender = tokio::sync::mpsc::Sender<chat::HandleMessage>;

pub struct Noalbs {
    pub state: UserState,
    pub chat_sender: ChatSender,

    // does this really need to be an option?
    pub switcher_handler: Option<tokio::task::JoinHandle<()>>,

    /// Used to save the config
    storage: Box<dyn config::ConfigLogic>,
}

impl Noalbs {
    pub async fn new(storage: Box<dyn config::ConfigLogic>, chat_sender: ChatSender) -> Self {
        let config = storage.load().unwrap();
        info!("Loaded {:?}", config.user);

        let mut state = State {
            config,
            switcher_state: state::SwitcherState::default(),
            broadcasting_software: state::BroadcastingSoftwareState::default(),
        };

        state.set_all_switchable_scenes();
        state.broadcasting_software.prev_scene =
            state.config.switcher.switching_scenes.normal.to_owned();

        let state = Arc::new(RwLock::new(state));

        {
            let mut w_state = state.write().await;

            let connection = match w_state.config.software {
                config::SoftwareConnection::Obs(ref obs_conf) => {
                    Obs::new(obs_conf.clone(), state.clone())
                }
            };

            // Do i need this option here?
            w_state.broadcasting_software.connection = Some(Box::new(connection));
        }

        let mut user = Self {
            state,
            chat_sender,
            switcher_handler: None,
            storage,
        };

        user.start_switcher().await;

        user
    }

    pub async fn add_stream_server(&self, stream_server: stream_servers::StreamServer) {
        let mut state = self.state.write().await;
        state.config.switcher.add_stream_server(stream_server);
    }

    /// Runs a new switcher
    pub async fn start_switcher(&mut self) {
        let user = { self.state.read().await.config.user.name.to_owned() };

        let span = tracing::span!(tracing::Level::INFO, "NOALBS", %user);
        let _enter = span.enter();

        let switcher = Some(Switcher::run(Switcher {
            state: self.state.clone(),
            chat_sender: self.chat_sender.clone(),
        }));

        self.switcher_handler = switcher;
    }

    pub async fn stop(&self) {
        let mut state = self.state.write().await;
        println!("> Stopping NOALBS {}", state.config.user.name);
        state.broadcasting_software.connection = None;

        if let Some(handler) = &self.switcher_handler {
            info!("Stopping switcher");
            handler.abort();
        }
    }

    pub async fn save_config(&self) -> Result<(), error::Error> {
        let state = self.state.read().await;
        self.storage.save(&state.config)
    }

    pub async fn contains_alias(&self, alias: &str) -> Result<bool, error::Error> {
        let state = self.state.read().await;
        let chat = &state.config.chat.as_ref().ok_or(error::Error::NoChat)?;
        let commands = &chat.commands;

        if commands.is_none() {
            return Ok(false);
        }

        let commands = commands.as_ref().unwrap();

        let contains = commands.iter().any(|(_, v)| match &v.alias {
            Some(vec_alias) => vec_alias.iter().any(|a| a == alias),
            None => false,
        });

        Ok(contains)
    }

    pub async fn add_alias(
        &self,
        alias: String,
        command: chat::Command,
    ) -> Result<(), error::Error> {
        let mut state = self.state.write().await;
        let chat = state.config.chat.as_mut().ok_or(error::Error::NoChat)?;

        let commands = chat.commands.get_or_insert(HashMap::new());
        let command = commands.entry(command).or_insert(config::CommandInfo {
            permission: None,
            alias: Some(Vec::new()),
        });

        if command.alias.is_none() {
            command.alias = Some(Vec::new());
        }

        command.alias.as_mut().unwrap().push(alias);

        Ok(())
    }

    pub async fn remove_alias(&self, alias: &str) -> Result<bool, error::Error> {
        let mut state = self.state.write().await;
        let chat = state.config.chat.as_mut().ok_or(error::Error::NoChat)?;

        let commands = match &mut chat.commands {
            Some(c) => c,
            None => return Ok(false),
        };

        let command = commands.iter_mut().find_map(|(_, value)| {
            if let Some(aliases) = &value.alias {
                if aliases.iter().any(|x| x == alias) {
                    return Some(value);
                }
            }

            None
        });

        let command = match command {
            Some(c) => c,
            None => return Ok(false),
        };

        let aliases = match &mut command.alias {
            Some(a) => a,
            None => return Ok(false),
        };

        if let Some(index) = aliases.iter().position(|v| *v == alias) {
            aliases.swap_remove(index);

            return Ok(true);
        }

        Ok(false)
    }

    pub async fn get_trigger_by_type(&self, kind: switcher::TriggerType) -> Option<u32> {
        let state = &self.state.read().await;
        let triggers = &state.config.switcher.triggers;

        match kind {
            switcher::TriggerType::Low => triggers.low,
            switcher::TriggerType::Rtt => triggers.rtt,
            switcher::TriggerType::Offline => triggers.offline,
        }
    }

    pub async fn update_trigger(&self, kind: switcher::TriggerType, value: u32) -> Option<u32> {
        let mut state = self.state.write().await;
        let triggers = &mut state.config.switcher.triggers;

        let real_value = if value == 0 { None } else { Some(value) };

        match kind {
            switcher::TriggerType::Low => triggers.low = real_value,
            switcher::TriggerType::Rtt => triggers.rtt = real_value,
            switcher::TriggerType::Offline => triggers.offline = real_value,
        }

        real_value
    }

    pub async fn get_autostop(&self) -> Result<bool, error::Error> {
        let state = &self.state.read().await;
        let chat = &state.config.chat.as_ref().ok_or(error::Error::NoChat)?;

        Ok(chat.enable_auto_stop_stream_on_host_or_raid)
    }

    pub async fn set_autostop(&self, enabled: bool) -> Result<(), error::Error> {
        let mut state = self.state.write().await;
        let chat = state.config.chat.as_mut().ok_or(error::Error::NoChat)?;

        chat.enable_auto_stop_stream_on_host_or_raid = enabled;

        Ok(())
    }

    pub async fn get_notify(&self) -> bool {
        let state = self.state.read().await;

        state.config.switcher.auto_switch_notification
    }

    pub async fn set_notify(&self, enabled: bool) {
        let mut state = self.state.write().await;

        state.config.switcher.auto_switch_notification = enabled;
    }

    pub async fn set_prefix(&self, prefix: String) -> Result<(), error::Error> {
        let mut state = self.state.write().await;
        let chat = state.config.chat.as_mut().ok_or(error::Error::NoChat)?;

        chat.prefix = prefix;

        Ok(())
    }

    pub async fn set_bitrate_switcher_state(&self, enabled: bool) {
        let mut state = self.state.write().await;

        state.config.switcher.set_bitrate_switcher_enabled(enabled);

        if enabled {
            state
                .switcher_state
                .switcher_enabled_notifier()
                .notify_waiters();
        }
    }
}

impl Drop for Noalbs {
    // Abort the switcher spawned task to stop it
    fn drop(&mut self) {
        if let Some(handler) = &self.switcher_handler {
            handler.abort();
        }
    }
}
