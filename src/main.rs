use echo::config::Config;

fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    println!("Echo — vector terminal UI");
    println!("Qdrant URL: {}", config.qdrant_url);
    println!("Embedding URL: {}", config.embedding_url);
    println!("Model: {}", config.embedding_model);
    Ok(())
}

