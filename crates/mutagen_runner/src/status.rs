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
    #[serde(default)]
    pub staging_progress: Option<StagingProgress>,
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

    /// Returns staging progress from whichever endpoint is currently staging.
    pub fn staging_progress(&self) -> Option<&StagingProgress> {
        self.alpha.staging_progress.as_ref()
            .or(self.beta.staging_progress.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_session_watching() {
        let json = r#"[{
            "session": {},
            "identifier": "abc123",
            "name": "frontend-12345678",
            "status": "watching",
            "successfulCycles": 42,
            "alpha": {
                "protocol": "local",
                "path": "/Users/tim/project",
                "connected": true,
                "scanned": true,
                "directories": 150,
                "files": 1200,
                "totalFileSize": 50000000
            },
            "beta": {
                "protocol": "docker",
                "path": "/app",
                "host": "container-id",
                "user": "root",
                "connected": true,
                "scanned": true,
                "directories": 150,
                "files": 1200,
                "totalFileSize": 50000000
            }
        }]"#;

        let sessions: Vec<MutagenSession> = serde_json::from_str(json).unwrap();
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.name, "frontend-12345678");
        assert_eq!(s.status, "watching");
        assert_eq!(s.successful_cycles, 42);
        assert!(s.alpha.staging_progress.is_none());
        assert!(s.beta.staging_progress.is_none());
        assert!(s.staging_progress().is_none());
    }

    #[test]
    fn test_deserialize_session_staging_beta() {
        let json = r#"[{
            "identifier": "abc123",
            "name": "allother-12345678",
            "status": "staging-beta",
            "alpha": {
                "protocol": "local",
                "path": "/Users/tim/project",
                "connected": true,
                "scanned": true,
                "directories": 500,
                "files": 81272,
                "totalFileSize": 900000000
            },
            "beta": {
                "protocol": "docker",
                "path": "/app",
                "host": "container-id",
                "connected": true,
                "scanned": true,
                "directories": 200,
                "files": 40000,
                "totalFileSize": 300000000,
                "stagingProgress": {
                    "path": ".phpstan/cache/PHPStan/08/29/082930abcdef.php",
                    "receivedFiles": 11793,
                    "expectedFiles": 81272,
                    "receivedSize": 17834,
                    "expectedSize": 17834,
                    "totalReceivedSize": 228580573
                }
            }
        }]"#;

        let sessions: Vec<MutagenSession> = serde_json::from_str(json).unwrap();
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.status, "staging-beta");

        // Alpha has no staging progress
        assert!(s.alpha.staging_progress.is_none());

        // Beta has staging progress
        let sp = s.beta.staging_progress.as_ref().unwrap();
        assert_eq!(sp.path, ".phpstan/cache/PHPStan/08/29/082930abcdef.php");
        assert_eq!(sp.received_files, 11793);
        assert_eq!(sp.expected_files, 81272);
        assert_eq!(sp.received_size, 17834);
        assert_eq!(sp.expected_size, 17834);
        assert_eq!(sp.total_received_size, 228580573);

        // Helper method should return beta's progress
        let progress = s.staging_progress().unwrap();
        assert_eq!(progress.received_files, 11793);
    }

    #[test]
    fn test_deserialize_session_staging_alpha() {
        let json = r#"[{
            "identifier": "abc123",
            "name": "src-12345678",
            "status": "staging-alpha",
            "alpha": {
                "protocol": "local",
                "path": "/Users/tim/project",
                "connected": true,
                "scanned": true,
                "stagingProgress": {
                    "path": "src/main.rs",
                    "receivedFiles": 50,
                    "expectedFiles": 200,
                    "receivedSize": 1024,
                    "expectedSize": 1024,
                    "totalReceivedSize": 50000
                }
            },
            "beta": {
                "protocol": "docker",
                "path": "/app",
                "host": "container-id",
                "connected": true,
                "scanned": true
            }
        }]"#;

        let sessions: Vec<MutagenSession> = serde_json::from_str(json).unwrap();
        let s = &sessions[0];
        assert_eq!(s.status, "staging-alpha");

        let sp = s.alpha.staging_progress.as_ref().unwrap();
        assert_eq!(sp.path, "src/main.rs");
        assert_eq!(sp.received_files, 50);
        assert_eq!(sp.expected_files, 200);

        assert!(s.beta.staging_progress.is_none());

        // Helper should return alpha's progress
        let progress = s.staging_progress().unwrap();
        assert_eq!(progress.path, "src/main.rs");
    }

    #[test]
    fn test_deserialize_ignores_unknown_fields() {
        // Mutagen JSON contains many fields we don't parse - ensure they don't break deserialization
        let json = r#"[{
            "identifier": "abc123",
            "name": "test-aabb",
            "status": "watching",
            "successfulCycles": 1,
            "conflicts": [],
            "excludedConflicts": 0,
            "alpha": {
                "protocol": "local",
                "path": "/test",
                "connected": true,
                "scanned": true,
                "symbolicLinks": 5,
                "scanProblems": [],
                "excludedScanProblems": 0,
                "transitionProblems": [],
                "excludedTransitionProblems": 0
            },
            "beta": {
                "protocol": "docker",
                "path": "/app",
                "host": "abc",
                "connected": true,
                "scanned": true
            }
        }]"#;

        let sessions: Vec<MutagenSession> = serde_json::from_str(json).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "test-aabb");
    }
}
