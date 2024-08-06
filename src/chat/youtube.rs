use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task;
use tokio::time::{self, Duration};
use youtube_chat::live_chat::LiveChatClientBuilder;
use youtube_chat::item::MessageItem;

use noalbs::chat::{ChatMessage, ChatSender};
use tracing::{debug, error, info};

pub struct YoutubeChat {
    yt_channel_id: String,
    live_chat: Arc<Mutex<LiveChatClient<impl Fn(String), impl Fn(), impl Fn(youtube_chat::item::ChatItem), impl Fn(anyhow::Error)>>>,
}

impl YoutubeChat {
    pub fn new(yt_channel_id: String, chat_tx: ChatSender) -> Self {
        let live_chat = LiveChatClientBuilder::new()
            .channel_id(yt_channel_id.clone())
            .on_start(|_live_id| {
                debug!("YouTube live chat started");
            })
            .on_error(|err| {
                error!("YouTube live chat error: {:?}", err);
            })
            .on_chat(move |chat_item| {
                let author_name = chat_item.author.name.unwrap_or("Unknown".to_string());
                let message_content: String = chat_item.message.iter().map(|m| match m {
                    MessageItem::Text(text) => text.clone(),
                    _ => "".to_string(),
                }).collect();

                info!("{}: {}", author_name, message_content);

                let chat_message = ChatMessage::new(
                    "Youtube".to_string(),
                    "Public".to_string(),
                    author_name.clone(),
                    author_name,
                    message_content,
                );

                // Send the chat message through the channel
                let _ = chat_tx.try_send(chat_message);
            })
            .on_end(|| {
                debug!("YouTube live chat ended");
            })
            .build();

        YoutubeChat {
            yt_channel_id,
            live_chat: Arc::new(Mutex::new(live_chat)),
        }
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
