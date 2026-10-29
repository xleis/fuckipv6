use anyhow::Result;

pub fn init_default() -> Result<()> {
    fuckipv6_common::init_logging_default()
}

pub fn init_with_level(level: &str) -> Result<()> {
    fuckipv6_common::init_logging_with_level(level)
}
