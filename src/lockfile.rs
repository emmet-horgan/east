use std::collections::BTreeMap;

use serde::{Serialize, Deserialize};
use url::Url;

#[derive(Debug, Serialize, Deserialize)]
pub struct Lockfile {
    #[serde(rename = "dep")]
    pub deps: BTreeMap<String, LockedDep>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LockedDep {
    pub git: Url,
    pub rev: String,
    pub path: String,
}


impl Lockfile {
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }
}
