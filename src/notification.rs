use eyre::Result;
use std::env;

pub struct Notification {
    pub url: Option<String>,
    pub message: String,
}

pub trait Sendable {
    async fn send(&self) -> Result<()>;
}

impl Sendable for Notification {
    async fn send(&self) -> Result<()> {
        let ntfy_url = env::var("NTFY_URL").expect("Missing NTFY_URL");
        let ntfy_topic = env::var("NTFY_TOPIC").expect("Missing NTFY_TOPIC");
        let ntfy_token = env::var("NTFY_TOKEN").expect("Missing NTFY_TOKEN");

        let client = reqwest::Client::new();
        client
            .post(format!("{}/{}", ntfy_url, ntfy_topic))
            .body(self.message.clone())
            .header("Authorization", format!("Bearer {}", ntfy_token))
            .header(
                "Actions",
                if self.url.is_some() {
                    format!("view, Explorer, {}, clear=true", self.url.as_ref().unwrap())
                } else {
                    "".to_string()
                },
            )
            .send()
            .await?;
        Ok(())
    }
}
