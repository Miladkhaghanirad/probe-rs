use std::fs::File;
use std::io::Write;

lazy_static::lazy_static! {
    pub static ref CONFIG: Config = match Config::new() {
        Ok(config) => config,
        Err(_) => Config::default(),
    };
}

const CONFIG_PATH: &str = ".config/probe-rs/targets/config.toml";

#[derive(Debug, Deserialize, Default)]
pub struct Config {
}

#[derive(Debug)]
pub enum Error {
    Config(config::ConfigError),
    Io(std::io::Error),
}

impl From<config::ConfigError> for Error {
    fn from(value: config::ConfigError) -> Self {
        Error::Config(value)
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Error::Io(value)
    }
}

impl Config {
    pub fn new() -> Result<Self, Error> {
        let mut s = config::Config::new();

        // Check if we can find the home dir and if so, load the config.
        if let Some(config) = dirs::home_dir().map(|home| home.join(CONFIG_PATH)) {
            // If no config file exists, try creating it.
            if !std::path::Path::new(&config).exists() {
                let mut f = File::create(&config)?;
                f.write_all(include_bytes!("../config/default.toml"))?;
            }

            // Try loading the configuration from the home directory.
            s.merge(config::File::with_name(&config.as_path().to_string_lossy()))?;

            // Load the entire config.
            s.try_into().map_err(From::from)
        } else {
            // If we can't load the config, load the default one.
            Ok(Default::default())
        }
    }
}