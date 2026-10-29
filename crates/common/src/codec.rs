use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    postcard::to_stdvec(value).context("postcard encode failed")
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let (value, remaining) = postcard::take_from_bytes(bytes).context("postcard decode failed")?;

    if !remaining.is_empty() {
        anyhow::bail!(
            "postcard decode consumed {} of {} bytes",
            bytes.len() - remaining.len(),
            bytes.len()
        );
    }

    Ok(value)
}
