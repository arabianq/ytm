mod gui;

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

    let _args = Args::parse();

    gui::run()
}
