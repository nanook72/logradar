use tokio::process::Command;
use tokio::sync::mpsc;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DockerContainer {
    pub name: String,
    pub image: String,
    pub status: String,
    pub id: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AzureContainerApp {
    pub name: String,
    pub resource_group: String,
    pub subscription_id: String,
    pub provisioning_state: String,
}

#[derive(Debug)]
pub enum DiscoveryResult {
    Docker(Result<Vec<DockerContainer>, String>),
    Azure(Result<Vec<AzureContainerApp>, String>),
    AzureToken(Result<String, String>),
}

pub fn discover_docker(tx: mpsc::Sender<DiscoveryResult>) {
    tokio::spawn(async move {
        let result = run_docker_discovery().await;
        let _ = tx.send(DiscoveryResult::Docker(result)).await;
    });
}

async fn run_docker_discovery() -> Result<Vec<DockerContainer>, String> {
    let output = Command::new("docker")
        .args(["ps", "--format", "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}"])
        .output()
        .await
        .map_err(|e| format!("docker not found: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("docker ps failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let containers: Vec<DockerContainer> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, '\t').collect();
            if parts.len() >= 4 {
                Some(DockerContainer {
                    id: parts[0].to_string(),
                    name: parts[1].to_string(),
                    image: parts[2].to_string(),
                    status: parts[3].to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(containers)
}

pub fn discover_azure(tx: mpsc::Sender<DiscoveryResult>) {
    let tx2 = tx.clone();
    // Fetch app list and access token in parallel
    tokio::spawn(async move {
        let result = run_azure_discovery().await;
        let _ = tx.send(DiscoveryResult::Azure(result)).await;
    });
    tokio::spawn(async move {
        let result = fetch_azure_token().await;
        let _ = tx2.send(DiscoveryResult::AzureToken(result)).await;
    });
}

async fn run_azure_discovery() -> Result<Vec<AzureContainerApp>, String> {
    let output = Command::new("az")
        .args(["containerapp", "list", "-o", "json"])
        .output()
        .await
        .map_err(|e| format!("az CLI not found: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("az containerapp list failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).map_err(|e| format!("Failed to parse JSON: {}", e))?;

    let apps = json
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?.to_string();
            let rg = item
                .get("resourceGroup")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Parse subscription ID from resource id:
            // /subscriptions/{sub}/resourceGroups/{rg}/providers/...
            let sub = item
                .get("id")
                .and_then(|v| v.as_str())
                .and_then(|id| {
                    let parts: Vec<&str> = id.split('/').collect();
                    // /subscriptions/{sub}/...  → parts[2]
                    parts.get(2).map(|s| s.to_string())
                })
                .unwrap_or_default();
            let state = item
                .get("properties")
                .and_then(|p| p.get("provisioningState"))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string();
            Some(AzureContainerApp {
                name,
                resource_group: rg,
                subscription_id: sub,
                provisioning_state: state,
            })
        })
        .collect();

    Ok(apps)
}

/// Fetch Azure management access token (runs `az account get-access-token`).
/// This is called in parallel with discovery so the token is ready when
/// log streaming starts — avoids a second az CLI startup.
async fn fetch_azure_token() -> Result<String, String> {
    let output = Command::new("az")
        .args([
            "account",
            "get-access-token",
            "--resource",
            "https://management.azure.com/",
            "-o",
            "json",
        ])
        .output()
        .await
        .map_err(|e| format!("az token: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("az token failed: {}", stderr.trim()));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("token parse: {}", e))?;

    json.get("accessToken")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "no accessToken in response".to_string())
}
