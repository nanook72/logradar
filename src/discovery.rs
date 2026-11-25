use tokio::process::Command;

use crate::util::sanitize;
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

#[derive(Debug, Clone)]
pub struct KubePod {
    pub namespace: String,
    pub name: String,
    pub phase: String,
}

#[derive(Debug)]
pub enum DiscoveryResult {
    Docker(Result<Vec<DockerContainer>, String>),
    Kube(Result<Vec<KubePod>, String>),
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
        return Err(format!("docker ps failed: {}", sanitize(stderr.trim())));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let containers: Vec<DockerContainer> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(4, '\t').collect();
            if parts.len() >= 4 {
                Some(DockerContainer {
                    id: sanitize(parts[0]),
                    name: sanitize(parts[1]),
                    image: sanitize(parts[2]),
                    status: sanitize(parts[3]),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(containers)
}

pub fn discover_kube(tx: mpsc::Sender<DiscoveryResult>) {
    tokio::spawn(async move {
        let result = run_kube_discovery().await;
        let _ = tx.send(DiscoveryResult::Kube(result)).await;
    });
}

async fn run_kube_discovery() -> Result<Vec<KubePod>, String> {
    let output = Command::new("kubectl")
        .args(["get", "pods", "--all-namespaces", "--output=json"])
        .output()
        .await
        .map_err(|e| format!("kubectl not found: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("kubectl get pods failed: {}", sanitize(stderr.trim())));
    }
    parse_kube_pods(&output.stdout)
}

/// Parse `kubectl get pods -o json`. Running pods first, then by namespace/name.
pub fn parse_kube_pods(json: &[u8]) -> Result<Vec<KubePod>, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("Failed to parse JSON: {}", e))?;
    let mut pods: Vec<KubePod> = v
        .get("items")
        .and_then(|i| i.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let meta = item.get("metadata")?;
                    Some(KubePod {
                        namespace: sanitize(meta.get("namespace")?.as_str()?),
                        name: sanitize(meta.get("name")?.as_str()?),
                        phase: item
                            .pointer("/status/phase")
                            .and_then(|p| p.as_str())
                            .map(sanitize)
                            .unwrap_or_else(|| "Unknown".into()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    pods.sort_by(|a, b| {
        (a.phase != "Running")
            .cmp(&(b.phase != "Running"))
            .then(a.namespace.cmp(&b.namespace))
            .then(a.name.cmp(&b.name))
    });
    Ok(pods)
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
        return Err(format!("az containerapp list failed: {}", sanitize(stderr.trim())));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).map_err(|e| format!("Failed to parse JSON: {}", e))?;

    let apps = json
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|item| {
            let name = sanitize(item.get("name")?.as_str()?);
            let rg = item
                .get("resourceGroup")
                .and_then(|v| v.as_str())
                .map(sanitize)
                .unwrap_or_default();
            // Parse subscription ID from resource id:
            // /subscriptions/{sub}/resourceGroups/{rg}/providers/...
            let sub = item
                .get("id")
                .and_then(|v| v.as_str())
                .and_then(|id| {
                    let parts: Vec<&str> = id.split('/').collect();
                    // /subscriptions/{sub}/...  → parts[2]
                    parts.get(2).map(|s| sanitize(s))
                })
                .unwrap_or_default();
            let state = item
                .get("properties")
                .and_then(|p| p.get("provisioningState"))
                .and_then(|v| v.as_str())
                .map(sanitize)
                .unwrap_or_else(|| "Unknown".to_string());
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
        return Err(format!("az token failed: {}", sanitize(stderr.trim())));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("token parse: {}", e))?;

    json.get("accessToken")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "no accessToken in response".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kube_pods_parse_and_sort() {
        let json = br#"{"items":[
            {"metadata":{"namespace":"prod","name":"api-2"},"status":{"phase":"Pending"}},
            {"metadata":{"namespace":"prod","name":"api-1"},"status":{"phase":"Running"}},
            {"metadata":{"namespace":"dev","name":"web\u001b]0;x\u0007"},"status":{"phase":"Running"}},
            {"metadata":{"name":"no-namespace"}}
        ]}"#;
        let pods = parse_kube_pods(json).unwrap();
        let names: Vec<String> = pods.iter().map(|p| format!("{}/{} {}", p.namespace, p.name, p.phase)).collect();
        assert_eq!(names, ["dev/web Running", "prod/api-1 Running", "prod/api-2 Pending"]);
        assert!(parse_kube_pods(b"not json").is_err());
    }
}
