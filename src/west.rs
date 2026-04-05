use serde::Deserialize;
use url::Url;

#[derive(Debug, Deserialize)]
pub struct Remote {
    name: String,
    #[serde(rename = "url-base")]
    url_base: Url 
}

#[derive(Debug, Deserialize)]
pub struct Defaults {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default = "default_revision")]
    revision: String
}

fn default_revision() -> String {
    "master".to_string()
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            remote: None,
            revision: "master".to_string()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Project {
    name: String,

    #[serde(default)]
    remote: Option<String>,

    #[serde(rename = "repo-path")]
    #[serde(default)]
    repo_path: Option<String>,

    #[serde(default)]
    url: Option<Url>,

    #[serde(default)]
    revision: Option<String>,
}


#[derive(Debug, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    defaults: Defaults,
    #[serde(default)]
    remotes: Vec<Remote>,
    #[serde(default)]
    projects: Vec<Project>
}

#[derive(Debug, Deserialize)]
pub struct West {
    manifest: Manifest
}


#[cfg(test)]
mod tests {
    use super::*;

    fn get_zephyr_west_yml() -> &'static str {
        include_str!("../tests/zephyr.west.yml")
    }

    #[test]
    fn zephyr_west_yml_parse() {
        let test_cfg = get_zephyr_west_yml();

        let _parsed: West = serde_yaml::from_str(&test_cfg)
            .expect("parsing failed");

        println!("{:?}", _parsed);
    }
}