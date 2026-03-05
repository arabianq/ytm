mod gui;
mod misc;

use anyhow::Result;
use clap::Parser;

#[macro_use]
extern crate rust_i18n;

i18n!("locales");

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {}

fn main() -> Result<()> {
    env_logger::init();

    if let Err(e) = dotenv::dotenv() {
        log::warn!("WARNING: Failed to load .env file: {e}");
    }

    let _args = Args::parse();

    gui::run()
}
