use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutagenSession {
    pub identifier: String,
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub successful_cycles: u64,
    pub alpha: EndpointStatus,
    pub beta: EndpointStatus,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointStatus {
    pub protocol: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub scanned: bool,
    #[serde(default)]
    pub directories: u64,
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub total_file_size: u64,
}

impl MutagenSession {
    pub fn beta_display(&self) -> String {
        if let Some(host) = &self.beta.host {
            let user = self.beta.user.as_deref().unwrap_or("");
            if user.is_empty() {
                format!("{}://{}{}", self.beta.protocol, host, self.beta.path)
            } else {
                format!("{}://{}@{}{}", self.beta.protocol, user, host, self.beta.path)
            }
        } else {
            self.beta.path.clone()
        }
    }
}
