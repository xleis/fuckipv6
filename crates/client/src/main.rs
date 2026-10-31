use anyhow::Result;

fn main() -> Result<()> {
    if let Some(exit_code) = fuckipv6_client::runtime_process::internal_entrypoint() {
        std::process::exit(exit_code);
    }
    fuckipv6_client::cli::run_entrypoint()
}
