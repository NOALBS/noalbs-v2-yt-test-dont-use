use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::{task, time};
use tracing::{info, error, debug};
use youtube_chat::LiveChat;
use tokio_stream::StreamExt;
use crate::chat::{self, ChatPlatform, HandleMessage, ChatSender};

pub struct YouTube {
    live_chat: LiveChat,
    chat_handler_tx: ChatSender,
}

impl YouTube {
    pub async fn new(chat_handler_tx: ChatSender) -> Result<Self, Box<dyn std::error::Error>> {
        let yt_channel_id = std::env::var("YOUTUBE_CHANNEL_ID")?;
        let live_chat = LiveChat::new(yt_channel_id).await?;
        Ok(Self {
            live_chat,
            chat_handler_tx,
        })
    }

    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let process_messages = Arc::new(Mutex::new(false));
        let process_messages_clone = process_messages.clone();

        let chat_handler_tx = self.chat_handler_tx.clone();
        let live_chat = self.live_chat.clone();

        let chat_handle = task::spawn(async move {
            let mut chat_stream = live_chat.chat_stream();
            while let Some(chat_item) = chat_stream.next().await {
                if *process_messages.lock().unwrap() {
                    debug!("{}: {}", chat_item.author_name, chat_item.message);
                    let message = HandleMessage::ChatMessage(chat::ChatMessage {
                        platform: ChatPlatform::YouTube,
                        permission: chat::Permission::Public,
                        channel: chat_item.author_name.clone(),
                        sender: chat_item.author_name.clone(),
                        message: chat_item.message.clone(),
                    });
                    chat_handler_tx.send(message).await.unwrap();
                }
            }
        });

        let start_handle = task::spawn(async move {
            time::sleep(Duration::from_secs(5)).await;
            *process_messages_clone.lock().unwrap() = true;
            info!("Started processing new messages");
        });

        tokio::try_join!(chat_handle, start_handle)?;
        Ok(())
    }
}
