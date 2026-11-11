//! Development entry point. The installed binary is `llmtodo server run`.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut config = llmtodo_server::Config::new(llmtodo_server::default_data_dir());
    if let Some(port) = std::env::var("LLMTODO_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
    {
        config.port = port;
    }
    let _guard = llmtodo_server::init_logging(&config.data_dir);
    llmtodo_server::run(config).await
}
