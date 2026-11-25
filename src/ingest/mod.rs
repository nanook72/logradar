use anyhow::Result;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceStatus {
    Starting,
    Running,
    Error(String),
    Stopped,
}

impl SourceStatus {
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        matches!(self, SourceStatus::Starting | SourceStatus::Running)
    }
}

pub enum SourceEvent {
    Log { source: String, line: String },
    Status { source: String, status: SourceStatus },
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub id: String,
    pub kind: String,
    pub status: SourceStatus,
}

// --- Docker source ---

pub fn spawn_docker(
    container: String,
    tx: mpsc::Sender<SourceEvent>,
) -> (SourceInfo, tokio::task::JoinHandle<()>) {
    let id = format!("docker/{}", container);
    let info = SourceInfo {
        id: id.clone(),
        kind: "docker".into(),
        status: SourceStatus::Starting,
    };
    let handle = tokio::spawn(async move {
        let _ = run_docker(&container, &id, tx).await;
    });
    (info, handle)
}

async fn run_docker(container: &str, source_id: &str, tx: mpsc::Sender<SourceEvent>) -> Result<()> {
    let result = Command::new("docker")
        .args(["logs", "-f", "--tail", "100", container])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn();

    let mut child = match result {
        Ok(child) => {
            let _ = tx.send(SourceEvent::Status {
                source: source_id.to_string(),
                status: SourceStatus::Running,
            }).await;
            child
        }
        Err(e) => {
            let _ = tx.send(SourceEvent::Status {
                source: source_id.to_string(),
                status: SourceStatus::Error(format!("docker: {}", e)),
            }).await;
            return Err(e.into());
        }
    };

    // Stream both stdout and stderr concurrently
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let tx2 = tx.clone();
    let sid = source_id.to_string();
    let sid2 = sid.clone();

    let stdout_task = tokio::spawn(async move {
        if let Some(out) = stdout {
            let mut lines = BufReader::new(out).lines();
            while let Some(line) = lines.next_line().await.unwrap_or(None) {
                if tx.send(SourceEvent::Log { source: sid.clone(), line }).await.is_err() {
                    break;
                }
            }
        }
    });

    let stderr_task = tokio::spawn(async move {
        if let Some(err) = stderr {
            let mut lines = BufReader::new(err).lines();
            while let Some(line) = lines.next_line().await.unwrap_or(None) {
                if tx2.send(SourceEvent::Log { source: sid2.clone(), line }).await.is_err() {
                    break;
                }
            }
        }
    });

    let _ = tokio::join!(stdout_task, stderr_task);

    // Child is held in scope — kill_on_drop ensures cleanup on task abort
    let _ = child.wait().await;
    Ok(())
}

// --- Command source ---

pub fn spawn_command(
    name: String,
    cmd: String,
    tx: mpsc::Sender<SourceEvent>,
) -> (SourceInfo, tokio::task::JoinHandle<()>) {
    let id = format!("cmd/{}", name);
    let info = SourceInfo {
        id: id.clone(),
        kind: "command".into(),
        status: SourceStatus::Starting,
    };
    let handle = tokio::spawn(async move {
        let _ = run_command(&cmd, &id, tx).await;
    });
    (info, handle)
}

async fn run_command(cmd: &str, source_id: &str, tx: mpsc::Sender<SourceEvent>) -> Result<()> {
    let result = Command::new("sh")
        .args(["-c", cmd])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn();

    let mut child = match result {
        Ok(child) => {
            let _ = tx.send(SourceEvent::Status {
                source: source_id.to_string(),
                status: SourceStatus::Running,
            }).await;
            child
        }
        Err(e) => {
            let _ = tx.send(SourceEvent::Status {
                source: source_id.to_string(),
                status: SourceStatus::Error(format!("command: {}", e)),
            }).await;
            return Err(e.into());
        }
    };

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            if tx
                .send(SourceEvent::Log {
                    source: source_id.to_string(),
                    line,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    }

    let _ = tx.send(SourceEvent::Status {
        source: source_id.to_string(),
        status: SourceStatus::Stopped,
    }).await;

    let _ = child.wait().await;
    Ok(())
}

// --- Azure Container App source ---

pub fn spawn_azure_containerapp(
    app_name: String,
    resource_group: String,
    subscription_id: String,
    token: Option<String>,
    tx: mpsc::Sender<SourceEvent>,
) -> (SourceInfo, tokio::task::JoinHandle<()>) {
    let id = format!("azure/{}", app_name);
    let info = SourceInfo {
        id: id.clone(),
        kind: "azure".into(),
        status: SourceStatus::Starting,
    };
    let handle = tokio::spawn(async move {
        let _ = run_azure_containerapp(&app_name, &resource_group, &subscription_id, token.as_deref(), &id, tx).await;
    });
    (info, handle)
}

async fn run_azure_containerapp(
    app_name: &str,
    resource_group: &str,
    subscription_id: &str,
    token: Option<&str>,
    source_id: &str,
    tx: mpsc::Sender<SourceEvent>,
) -> Result<()> {
    // Try the fast curl-based approach if we have a pre-fetched token
    if let Some(token) = token {
        if !subscription_id.is_empty() {
            match run_azure_fast(app_name, resource_group, subscription_id, token, source_id, &tx).await {
                Ok(()) => return Ok(()),
                Err(_) => {
                    // Fast path failed — fall through to az CLI
                }
            }
        }
    }

    // Fallback: use az CLI (slower but more reliable)
    run_azure_cli(app_name, resource_group, source_id, tx).await
}

/// Fast path: use curl + pre-fetched token to call Azure REST API directly.
/// Avoids the ~3-5s Python startup of the az CLI.
async fn run_azure_fast(
    app_name: &str,
    resource_group: &str,
    subscription_id: &str,
    token: &str,
    source_id: &str,
    tx: &mpsc::Sender<SourceEvent>,
) -> Result<()> {
    let base = format!(
        "https://management.azure.com/subscriptions/{}/resourceGroups/{}/providers/Microsoft.App/containerApps/{}",
        subscription_id, resource_group, app_name
    );
    let api = "api-version=2024-03-01";
    let auth_header = format!("Authorization: Bearer {}", token);

    // Step 1: Get app details (need managedEnvironmentId for the log stream domain)
    let app_json = curl_get_json(&format!("{}?{}", base, api), &auth_header).await?;
    let env_id = app_json
        .pointer("/properties/managedEnvironmentId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("no managedEnvironmentId"))?;

    // Step 2: Get environment domain
    let env_json = curl_get_json(
        &format!("https://management.azure.com{}?{}", env_id, api),
        &auth_header,
    )
    .await?;
    let env_domain = env_json
        .pointer("/properties/defaultDomain")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("no defaultDomain"))?
        .to_string();

    // Step 3: Get latest active revision
    let revisions_json = curl_get_json(
        &format!("{}/revisions?{}", base, api),
        &auth_header,
    )
    .await?;
    let latest_rev = revisions_json
        .pointer("/value/0/name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("no revisions"))?
        .to_string();

    // Step 4: Get first replica
    let replicas_json = curl_get_json(
        &format!("{}/revisions/{}/replicas?{}", base, latest_rev, api),
        &auth_header,
    )
    .await?;
    let replica = replicas_json
        .pointer("/value/0/name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("no replicas"))?
        .to_string();

    // Step 5: Get log stream auth token
    let auth_json = curl_post_json(
        &format!("{}/getAuthToken?{}", base, api),
        &auth_header,
    )
    .await?;
    let log_token = auth_json
        .pointer("/properties/token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("no log stream token"))?;

    // Step 6: Stream logs from the log stream endpoint
    let log_url = format!(
        "https://{}/subscriptions/{}/resourceGroups/{}/containerApps/{}/revisions/{}/replicas/{}/logstream?follow=true&tailLines=100&output=text",
        env_domain, subscription_id, resource_group, app_name, latest_rev, replica
    );

    let mut child = Command::new("curl")
        .args([
            "-N", "-s", "-f",
            "-H", &format!("Authorization: Bearer {}", log_token),
            &log_url,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut first = true;
        while let Some(line) = lines.next_line().await? {
            if first {
                let _ = tx.send(SourceEvent::Status {
                    source: source_id.to_string(),
                    status: SourceStatus::Running,
                }).await;
                first = false;
            }
            if tx
                .send(SourceEvent::Log {
                    source: source_id.to_string(),
                    line,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    }

    let _ = tx.send(SourceEvent::Status {
        source: source_id.to_string(),
        status: SourceStatus::Stopped,
    }).await;

    let _ = child.wait().await;
    Ok(())
}

/// Fallback: use az CLI for log streaming (slower due to Python startup).
async fn run_azure_cli(
    app_name: &str,
    resource_group: &str,
    source_id: &str,
    tx: mpsc::Sender<SourceEvent>,
) -> Result<()> {
    let result = Command::new("az")
        .args([
            "containerapp",
            "logs",
            "show",
            "-n",
            app_name,
            "-g",
            resource_group,
            "--type",
            "console",
            "--follow",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn();

    let mut child = match result {
        Ok(child) => child,
        Err(e) => {
            let _ = tx.send(SourceEvent::Status {
                source: source_id.to_string(),
                status: SourceStatus::Error(format!("az: {}", e)),
            }).await;
            return Err(e.into());
        }
    };

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let mut first = true;
        while let Some(line) = lines.next_line().await? {
            if first {
                let _ = tx.send(SourceEvent::Status {
                    source: source_id.to_string(),
                    status: SourceStatus::Running,
                }).await;
                first = false;
            }
            if tx
                .send(SourceEvent::Log {
                    source: source_id.to_string(),
                    line,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    }

    let _ = tx.send(SourceEvent::Status {
        source: source_id.to_string(),
        status: SourceStatus::Stopped,
    }).await;

    let _ = child.wait().await;
    Ok(())
}

/// Helper: GET a URL with auth header, parse response as JSON.
async fn curl_get_json(url: &str, auth_header: &str) -> Result<serde_json::Value> {
    let output = Command::new("curl")
        .args(["-sf", "-H", auth_header, url])
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow::anyhow!("curl GET failed for {}", url));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// Helper: POST to a URL with auth header, parse response as JSON.
async fn curl_post_json(url: &str, auth_header: &str) -> Result<serde_json::Value> {
    let output = Command::new("curl")
        .args([
            "-sf", "-X", "POST",
            "-H", auth_header,
            "-H", "Content-Type: application/json",
            url,
        ])
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow::anyhow!("curl POST failed for {}", url));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

// --- File tail source ---

pub fn spawn_file(
    path: String,
    tx: mpsc::Sender<SourceEvent>,
) -> (SourceInfo, tokio::task::JoinHandle<()>) {
    let id = format!("file/{}", path);
    let info = SourceInfo {
        id: id.clone(),
        kind: "file".into(),
        status: SourceStatus::Starting,
    };
    let handle = tokio::spawn(async move {
        let _ = run_file_tail(&path, &id, tx).await;
    });
    (info, handle)
}

async fn run_file_tail(path: &str, source_id: &str, tx: mpsc::Sender<SourceEvent>) -> Result<()> {
    let result = Command::new("tail")
        .args(["-f", "-n", "+1", path])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn();

    let mut child = match result {
        Ok(child) => {
            let _ = tx.send(SourceEvent::Status {
                source: source_id.to_string(),
                status: SourceStatus::Running,
            }).await;
            child
        }
        Err(e) => {
            let _ = tx.send(SourceEvent::Status {
                source: source_id.to_string(),
                status: SourceStatus::Error(format!("tail: {}", e)),
            }).await;
            return Err(e.into());
        }
    };

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Some(line) = lines.next_line().await? {
            if tx
                .send(SourceEvent::Log {
                    source: source_id.to_string(),
                    line,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    }

    let _ = tx.send(SourceEvent::Status {
        source: source_id.to_string(),
        status: SourceStatus::Stopped,
    }).await;

    let _ = child.wait().await;
    Ok(())
}
