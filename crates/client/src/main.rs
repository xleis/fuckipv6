use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // 解析CLI命令
    fuckipv6_client::cli::run().await
}
