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
    #[serde(default)]
    pub staging_progress: Option<StagingProgress>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagingProgress {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub received_files: u64,
    #[serde(default)]
    pub expected_files: u64,
    #[serde(default)]
    pub received_size: u64,
    #[serde(default)]
    pub expected_size: u64,
    #[serde(default)]
    pub total_received_size: u64,
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
