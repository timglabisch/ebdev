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
    pub fn status_display(&self) -> &'static str {
        match self.status.as_str() {
            "disconnected" => "Disconnected",
            "halted-on-root-empty" => "Halted (root empty)",
            "halted-on-root-deletion" => "Halted (root deleted)",
            "halted-on-root-type-change" => "Halted (root type changed)",
            "connecting-alpha" => "Connecting α",
            "connecting-beta" => "Connecting β",
            "watching" => "Watching ✓",
            "scanning" => "Scanning",
            "waiting-for-rescan" => "Waiting for rescan",
            "reconciling" => "Reconciling",
            "staging-alpha" => "Staging α",
            "staging-beta" => "Staging β",
            "transitioning" => "Transitioning",
            "saving" => "Saving",
            _ => "Unknown",
        }
    }

    pub fn is_syncing(&self) -> bool {
        matches!(
            self.status.as_str(),
            "scanning"
                | "reconciling"
                | "staging-alpha"
                | "staging-beta"
                | "transitioning"
                | "saving"
        )
    }

    pub fn is_connected(&self) -> bool {
        self.alpha.connected && self.beta.connected
    }

    pub fn is_complete(&self) -> bool {
        self.status == "watching" || self.status == "waiting-for-rescan"
    }

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
