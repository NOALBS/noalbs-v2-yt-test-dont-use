use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task;
use tokio::time::{self, Duration};
use youtube_chat::live_chat::{LiveChatClient, LiveChatClientBuilder};
use youtube_chat::item::{ChatItem, MessageItem};

use crate::{ChatSender, chat::{ChatMessage, ChatPlatform, Permission, HandleMessage, ChatLogic}};
use tracing::{debug, error, info};

pub struct YoutubeChat {
    live_chat: Arc<Mutex<LiveChatClient<
        Box<dyn Fn(String) + Send + Sync>,
        Box<dyn Fn() + Send + Sync>,
        Box<dyn Fn(ChatItem) + Send + Sync>,
        Box<dyn Fn(anyhow::Error) + Send + Sync>,
    >>>,
}

impl YoutubeChat {
    pub async fn new(yt_channel_id: String, chat_tx: ChatSender) -> Result<Self, anyhow::Error> {
        let live_chat = LiveChatClientBuilder::new()
            .channel_id(yt_channel_id.clone())
            .on_start(Box::new(|_live_id| {
                debug!("YouTube live chat started");
            }) as Box<dyn Fn(String) + Send + Sync>)
            .on_error(Box::new(|err| {
                error!("YouTube live chat error: {:?}", err);
            }) as Box<dyn Fn(anyhow::Error) + Send + Sync>)
            .on_chat(Box::new(move |chat_item: ChatItem| {
                let chat_tx = chat_tx.clone();
                let yt_channel_id = yt_channel_id.clone();
                let author_name = chat_item.author.name.clone().unwrap_or_else(|| "Unknown".to_string());
                let message_content: String = chat_item.message.iter().map(|m| match m {
                    MessageItem::Text(text) => text.clone(),
                    _ => "".to_string(),
                }).collect();

                info!("{}: {}", author_name, message_content);

                let permission = if chat_item.is_owner {
                    Permission::Admin
                } else if chat_item.is_moderator {
                    Permission::Mod
                } else {
                    Permission::Public
                };

                let chat_message = ChatMessage {
                    platform: ChatPlatform::Youtube,
                    permission,
                    sender: author_name.clone(),
                    message: message_content,
                    channel: yt_channel_id.clone(), // using cloned value
                };

                tokio::spawn(async move {
                    if let Err(e) = chat_tx.send(HandleMessage::ChatMessage(chat_message)).await {
                        error!("Failed to send chat message: {}", e);
                    }
                });
            }) as Box<dyn Fn(ChatItem) + Send + Sync>)
            .on_end(Box::new(|| {
                debug!("YouTube live chat ended");
            }) as Box<dyn Fn() + Send + Sync>)
            .build();

        Ok(Self {
            live_chat: Arc::new(Mutex::new(live_chat)),
        })
    }

    pub async fn start(&self) {
        let live_chat = self.live_chat.clone();
        let chat_handle = task::spawn(async move {
            let mut live_chat = live_chat.lock().await;
            live_chat.start().await.unwrap();
        });

        chat_handle.await.unwrap();

        let live_chat = self.live_chat.clone();
        let fetch_handle = task::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(3000));
            loop {
                interval.tick().await;
                let mut live_chat = live_chat.lock().await;
                live_chat.execute().await;
            }
        });

        fetch_handle.await.unwrap();
    }
}

#[async_trait::async_trait]
impl ChatLogic for YoutubeChat {
    async fn send_message(&self, _channel: String, _message: String) {
        // YouTube live chat API does not support sending messages directly
        // So we will leave this method unimplemented
        tracing::debug!("Sending messages to YouTube chat is not supported.");
    }
}
