use crate::model::*;
use anyhow::{Result, bail};
use async_trait::async_trait;
pub mod demo;
pub mod mastodon;

/// Network adapters translate their API into these common social primitives.
/// Unsupported optional operations fail explicitly instead of pretending to work.
#[async_trait]
pub trait SocialProvider: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    async fn me(&self) -> Result<Account>;
    async fn feed(&self, feed: &Feed, cursor: Option<&str>) -> Result<Page<Post>>;
    async fn notifications(&self, cursor: Option<&str>) -> Result<Page<Notification>>;
    async fn search(&self, query: &str) -> Result<SearchResults>;
    async fn context(&self, id: &str) -> Result<Thread>;
    async fn post(&self, id: &str) -> Result<Post>;
    async fn profile(&self, id: &str) -> Result<Account>;
    async fn relationship(&self, _id: &str) -> Result<Relationship> {
        Ok(Relationship::default())
    }
    async fn post_action(&self, id: &str, action: PostAction) -> Result<Option<Post>>;
    async fn account_action(&self, _id: &str, _action: AccountAction) -> Result<Relationship> {
        bail!("This network does not support account actions")
    }
    async fn publish(&self, compose: &Compose) -> Result<Post>;
    async fn source(&self, _id: &str) -> Result<Compose> {
        bail!("This network does not support editing")
    }
    async fn vote(&self, _id: &str, _choices: &[usize]) -> Result<Poll> {
        bail!("This network does not support polls")
    }
    async fn lists(&self) -> Result<Vec<SocialList>> {
        Ok(Vec::new())
    }
    async fn create_list(&self, _title: &str) -> Result<SocialList> {
        bail!("This network does not support lists")
    }
    async fn list_member(&self, _list: &str, _account: &str, _add: bool) -> Result<()> {
        bail!("This network does not support lists")
    }
}
