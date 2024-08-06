use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::{task, time};
use tracing::{info, debug, error};
use youtube_chat::live_chat::LiveChatClientBuilder;
use crate::{chat::{self, ChatPlatform, HandleMessage}, ChatSender};

pub struct YouTube<U, SF, ENF, CF, ERF> {
    live_chat: Arc<Mutex<LiveChatClientBuilder<U, SF, ENF, CF, ERF>>>,
    chat_handler_tx: ChatSender,
}

impl<U, SF, ENF, CF, ERF> YouTube<U, SF, ENF, CF, ERF> 
where
    U: Send + Sync + 'static,
    SF: Fn(U) + Send + Sync + 'static,
    ENF: Fn(std::io::Error) + Send + Sync + 'static,
    CF: Fn(chat::ChatMessage) + Send + Sync + 'static,
    ERF: Fn() + Send + Sync + 'static,
{
    pub async fn new(chat_handler_tx: ChatSender) -> Result<Self, Box<dyn std::error::Error>> {
        let yt_channel_id = std::env::var("YOUTUBE_CHANNEL_ID")?;
        let live_chat = LiveChatClientBuilder::new()
            .channel_id(yt_channel_id)
            .on_chat(move |chat_item| {
                let chat_handler_tx = chat_handler_tx.clone();
                task::spawn(async move {
                    debug!("{}: {}", chat_item.author_name, chat_item.message);
                    let message = HandleMessage::ChatMessage(chat::ChatMessage {
                        platform: ChatPlatform::Youtube,
                        permission: chat::Permission::Public,
                        channel: chat_item.author_name.clone(),
                        sender: chat_item.author_name.clone(),
                        message: chat_item.message.clone(),
                    });
                    if let Err(e) = chat_handler_tx.send(message).await {
                        error!("Failed to send chat message: {:?}", e);
                    }
                });
            })
            .on_error(|err| {
                error!("YouTube chat error: {:?}", err);
            })
            .build();

        Ok(Self {
            live_chat: Arc::new(Mutex::new(live_chat)),
            chat_handler_tx,
        })
    }

    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let process_messages = Arc::new(Mutex::new(false));
        let process_messages_clone = process_messages.clone();

        let live_chat = self.live_chat.clone();

        let chat_handle = task::spawn(async move {
            let mut live_chat = live_chat.lock().unwrap();
            live_chat.start().await.unwrap();

            let mut interval = time::interval(Duration::from_millis(3000));
            loop {
                interval.tick().await;
                live_chat.execute().await;
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
