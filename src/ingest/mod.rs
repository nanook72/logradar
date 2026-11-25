use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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

/// Longest line kept from a source; the rest of an overlong line is dropped.
/// Bounds memory when a source emits a huge line or one with no newline.
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// Read `reader` line by line and forward each line as a `SourceEvent::Log`.
/// With `announce_running`, the first line also marks the source Running
/// (for sources whose spawn succeeding doesn't mean the stream is up).
///
/// Lines are cut at `MAX_LINE_BYTES` and decoded lossily, so neither a huge
/// line nor a stray invalid UTF-8 byte can exhaust memory or end the stream.
async fn forward_lines<R>(reader: R, source_id: &str, tx: mpsc::Sender<SourceEvent>, announce_running: bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    forward_lines_with(reader, source_id, tx, announce_running, None).await
}

/// `forward_lines`, passing each line through `transform` first.
async fn forward_lines_with<R>(
    reader: R,
    source_id: &str,
    tx: mpsc::Sender<SourceEvent>,
    announce_running: bool,
    transform: Option<fn(String) -> String>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut buf: Vec<u8> = Vec::new();
    let mut truncated = false;
    let mut first = announce_running;

    loop {
        let (consumed, line_done, eof) = match reader.fill_buf().await {
            Ok([]) => (0, !buf.is_empty(), true),
            Ok(avail) => {
                let (chunk, consumed, line_done) = match avail.iter().position(|&b| b == b'\n') {
                    Some(i) => (&avail[..i], i + 1, true),
                    None => (avail, avail.len(), false),
                };
                let room = MAX_LINE_BYTES.saturating_sub(buf.len());
                if chunk.len() > room {
                    truncated = true;
                }
                buf.extend_from_slice(&chunk[..chunk.len().min(room)]);
                (consumed, line_done, false)
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => (0, !buf.is_empty(), true),
        };
        reader.consume(consumed);

        if line_done {
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            let mut line = String::from_utf8_lossy(&buf).into_owned();
            if truncated {
                line.push_str(" …[truncated]");
            }
            buf.clear();
            truncated = false;
            if let Some(f) = transform {
                line = f(line);
            }

            if first {
                let _ = tx
                    .send(SourceEvent::Status {
                        source: source_id.to_string(),
                        status: SourceStatus::Running,
                    })
                    .await;
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
                return;
            }
        }
        if eof {
            return;
        }
    }
}

/// Whether a source keeps streaming new lines or stops at the end of what is
/// there now (for `logradar summary`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follow {
    Yes,
    No,
}

// --- Docker source ---

pub fn spawn_docker(
    container: String,
    follow: Follow,
    tx: mpsc::Sender<SourceEvent>,
) -> (SourceInfo, tokio::task::JoinHandle<()>) {
    let id = format!("docker/{}", container);
    let info = SourceInfo {
        id: id.clone(),
        kind: "docker".into(),
        status: SourceStatus::Starting,
    };
    let handle = tokio::spawn(async move {
        let _ = run_docker(&container, follow, &id, tx).await;
    });
    (info, handle)
}

async fn run_docker(container: &str, follow: Follow, source_id: &str, tx: mpsc::Sender<SourceEvent>) -> Result<()> {
    let args: &[&str] = match follow {
        Follow::Yes => &["logs", "-f", "--tail", "100", "--"],
        Follow::No => &["logs", "--"],
    };
    let result = Command::new("docker")
        .args(args)
        .arg(container)
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
            forward_lines(out, &sid, tx, false).await;
        }
    });

    let stderr_task = tokio::spawn(async move {
        if let Some(err) = stderr {
            forward_lines(err, &sid2, tx2, false).await;
        }
    });

    let _ = tokio::join!(stdout_task, stderr_task);

    // Child is held in scope — kill_on_drop ensures cleanup on task abort
    let _ = child.wait().await;
    Ok(())
}

// --- Kubernetes source ---

/// What to read logs from, as written after `--kube`:
/// `pod`, `kind/name` (e.g. `deploy/api`) or a label selector (`app=api`),
/// optionally prefixed with `namespace:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubeTarget {
    pub namespace: Option<String>,
    pub what: KubeWhat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KubeWhat {
    /// A pod name or `kind/name`.
    Resource(String),
    /// A label selector such as `app=api,tier!=db`.
    Selector(String),
}

/// DNS-1123-ish names and resource kinds: what Kubernetes allows.
fn is_kube_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && !s.starts_with('-')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
}

impl KubeTarget {
    pub fn parse(spec: &str) -> Result<KubeTarget> {
        let (namespace, rest) = match spec.split_once(':') {
            Some((ns, rest)) => (Some(ns), rest),
            None => (None, spec),
        };
        if let Some(ns) = namespace {
            if !is_kube_name(ns) {
                anyhow::bail!("bad Kubernetes namespace '{}'", ns);
            }
        }
        let what = if rest.contains('=') {
            let ok = rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '=' | '!' | ','));
            if !ok || rest.starts_with('-') {
                anyhow::bail!("bad Kubernetes label selector '{}'", rest);
            }
            KubeWhat::Selector(rest.to_string())
        } else {
            let ok = match rest.split_once('/') {
                Some((kind, name)) => is_kube_name(kind) && is_kube_name(name),
                None => is_kube_name(rest),
            };
            if !ok {
                anyhow::bail!("bad Kubernetes target '{}' (use pod, kind/name or label=value)", rest);
            }
            KubeWhat::Resource(rest.to_string())
        };
        Ok(KubeTarget {
            namespace: namespace.map(str::to_string),
            what,
        })
    }

    /// Source id shown in the UI.
    pub fn id(&self) -> String {
        let what = match &self.what {
            KubeWhat::Resource(r) => r.clone(),
            KubeWhat::Selector(s) => format!("[{}]", s),
        };
        match &self.namespace {
            Some(ns) => format!("kube/{}/{}", ns, what),
            None => format!("kube/{}", what),
        }
    }

    /// `kubectl logs` arguments. Values use `--opt=value` and the resource
    /// comes after `--`, so nothing can be read as another option.
    pub fn kubectl_args(&self, follow: Follow) -> Vec<String> {
        let mut args = vec!["logs".to_string()];
        if follow == Follow::Yes {
            args.push("--follow".into());
            args.push("--tail=100".into());
        }
        args.push("--all-containers=true".into());
        // "[pod/name/container] " before each line, stripped again on read;
        // with several pods/containers it keeps lines from interleaving
        // mid-stream ambiguously in kubectl's own output.
        args.push("--prefix=true".into());
        if let Some(ns) = &self.namespace {
            args.push(format!("--namespace={}", ns));
        }
        match &self.what {
            KubeWhat::Selector(sel) => {
                args.push(format!("--selector={}", sel));
                args.push("--max-log-requests=20".into());
            }
            KubeWhat::Resource(r) => {
                args.push("--".into());
                args.push(r.clone());
            }
        }
        args
    }
}

/// Drop kubectl's `[pod/name/container] ` prefix so pod names (which change
/// with every rollout) don't split patterns.
fn strip_kube_prefix(line: String) -> String {
    if line.starts_with("[pod/") {
        if let Some(end) = line.find("] ") {
            return line[end + 2..].to_string();
        }
    }
    line
}

pub fn spawn_kube(
    target: KubeTarget,
    follow: Follow,
    tx: mpsc::Sender<SourceEvent>,
) -> (SourceInfo, tokio::task::JoinHandle<()>) {
    let id = target.id();
    let info = SourceInfo {
        id: id.clone(),
        kind: "kube".into(),
        status: SourceStatus::Starting,
    };
    let handle = tokio::spawn(async move {
        let child = Command::new("kubectl")
            .args(target.kubectl_args(follow))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let _ = tx
                    .send(SourceEvent::Status {
                        source: id.clone(),
                        status: SourceStatus::Error(format!("kubectl: {}", e)),
                    })
                    .await;
                return;
            }
        };
        if let Some(out) = child.stdout.take() {
            forward_lines_with(out, &id, tx.clone(), true, Some(strip_kube_prefix)).await;
        }
        let status = child.wait().await;
        let failed = !matches!(status, Ok(s) if s.success());
        let detail = if failed {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                use tokio::io::AsyncReadExt;
                let mut buf = vec![0u8; 4096];
                let n = e.read(&mut buf).await.unwrap_or(0);
                err = String::from_utf8_lossy(&buf[..n]).trim().to_string();
            }
            Some(crate::util::sanitize(if err.is_empty() { "kubectl exited with an error" } else { &err }))
        } else {
            None
        };
        let _ = tx
            .send(SourceEvent::Status {
                source: id,
                status: match detail {
                    Some(d) => SourceStatus::Error(d),
                    None => SourceStatus::Stopped,
                },
            })
            .await;
    });
    (info, handle)
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
        forward_lines(stdout, source_id, tx.clone(), false).await;
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
    for part in [subscription_id, resource_group, app_name] {
        if !is_safe_url_segment(part) {
            return Err(anyhow::anyhow!("unexpected characters in Azure resource name"));
        }
    }
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
    if !is_safe_resource_id(env_id) {
        return Err(anyhow::anyhow!("unexpected managedEnvironmentId"));
    }

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
    if !is_container_apps_domain(&env_domain) {
        return Err(anyhow::anyhow!("unexpected log stream domain"));
    }

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
    if !is_safe_url_segment(&latest_rev) {
        return Err(anyhow::anyhow!("unexpected revision name"));
    }

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
    if !is_safe_url_segment(&replica) {
        return Err(anyhow::anyhow!("unexpected replica name"));
    }

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

    let mut cmd = curl_command(&["-N", "-s", "-f"], &log_url);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = spawn_curl(cmd, &format!("Authorization: Bearer {}", log_token)).await?;

    if let Some(stdout) = child.stdout.take() {
        forward_lines(stdout, source_id, tx.clone(), true).await;
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
            &format!("--name={}", app_name),
            &format!("--resource-group={}", resource_group),
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
        forward_lines(stdout, source_id, tx.clone(), true).await;
    }

    let _ = tx.send(SourceEvent::Status {
        source: source_id.to_string(),
        status: SourceStatus::Stopped,
    }).await;

    let _ = child.wait().await;
    Ok(())
}

/// Build a curl invocation that sends `auth_header` to `url`. The header is
/// not put on the command line, where any local user could read the bearer
/// token from `ps` or /proc: curl reads it from a config file on stdin
/// (`-K -`), written by `spawn_curl`.
fn curl_command(flags: &[&str], url: &str) -> Command {
    let mut cmd = Command::new("curl");
    cmd.args(flags)
        .args(["-K", "-", "--", url])
        .stdin(std::process::Stdio::piped())
        .kill_on_drop(true);
    cmd
}

/// The curl config text carrying `auth_header`, quoted per curl's config syntax.
fn curl_config(auth_header: &str) -> String {
    format!("header = \"{}\"\n", curl_quote(auth_header))
}

/// Quote a value for a curl config file (`-K`): escape `\` and `"`, and drop
/// CR/LF so the value can't end its line and start a new directive.
pub fn curl_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            // A header can't span lines; never let one start a new config line.
            '\r' | '\n' => {}
            c => quoted.push(c),
        }
    }
    quoted
}

/// Spawn `cmd` (from `curl_command`) and hand it the header on stdin.
async fn spawn_curl(mut cmd: Command, auth_header: &str) -> Result<tokio::process::Child> {
    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(curl_config(auth_header).as_bytes()).await?;
        // Dropping stdin closes it, so curl starts the request.
    }
    Ok(child)
}

/// A path segment or name taken from Azure/discovery output that is
/// interpolated into a management URL: only the characters Azure allows in
/// resource names, so it can't add path segments, a query, or a userinfo host.
fn is_safe_url_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 260
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '(' | ')'))
        && s != "."
        && s != ".."
}

/// `managedEnvironmentId` must be a resource path under /subscriptions/.
fn is_safe_resource_id(id: &str) -> bool {
    id.strip_prefix("/subscriptions/")
        .map(|rest| rest.split('/').all(is_safe_url_segment))
        .unwrap_or(false)
}

/// The log-stream host must be an Azure Container Apps domain, so the
/// log-stream token is only ever sent to Azure.
fn is_container_apps_domain(host: &str) -> bool {
    host.ends_with(".azurecontainerapps.io")
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// Helper: GET a URL with auth header, parse response as JSON.
async fn curl_get_json(url: &str, auth_header: &str) -> Result<serde_json::Value> {
    let mut cmd = curl_command(&["-sf"], url);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let output = spawn_curl(cmd, auth_header).await?.wait_with_output().await?;
    if !output.status.success() {
        return Err(anyhow::anyhow!("curl GET failed for {}", url));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// Helper: POST to a URL with auth header, parse response as JSON.
async fn curl_post_json(url: &str, auth_header: &str) -> Result<serde_json::Value> {
    let mut cmd = curl_command(&["-sf", "-X", "POST", "-H", "Content-Type: application/json"], url);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let output = spawn_curl(cmd, auth_header).await?.wait_with_output().await?;
    if !output.status.success() {
        return Err(anyhow::anyhow!("curl POST failed for {}", url));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

// --- File tail source ---

/// Tail a file; the path `-` means standard input.
pub fn spawn_file(
    path: String,
    follow: Follow,
    tx: mpsc::Sender<SourceEvent>,
) -> (SourceInfo, tokio::task::JoinHandle<()>) {
    if path == "-" {
        return spawn_stdin(tx);
    }
    let id = format!("file/{}", path);
    let info = SourceInfo {
        id: id.clone(),
        kind: "file".into(),
        status: SourceStatus::Starting,
    };
    let handle = tokio::spawn(async move {
        let _ = match follow {
            Follow::Yes => run_file_tail(&path, &id, tx).await,
            Follow::No => run_file_once(&path, &id, tx).await,
        };
    });
    (info, handle)
}

/// Read a file to its end once.
async fn run_file_once(path: &str, source_id: &str, tx: mpsc::Sender<SourceEvent>) -> Result<()> {
    match tokio::fs::File::open(path).await {
        Ok(file) => {
            let _ = tx
                .send(SourceEvent::Status {
                    source: source_id.to_string(),
                    status: SourceStatus::Running,
                })
                .await;
            forward_lines(file, source_id, tx.clone(), false).await;
            let _ = tx
                .send(SourceEvent::Status {
                    source: source_id.to_string(),
                    status: SourceStatus::Stopped,
                })
                .await;
            Ok(())
        }
        Err(e) => {
            let _ = tx
                .send(SourceEvent::Status {
                    source: source_id.to_string(),
                    status: SourceStatus::Error(format!("{}: {}", path, e)),
                })
                .await;
            Err(e.into())
        }
    }
}

// --- Standard input ---

/// Read lines piped into logradar until EOF (`cmd | logradar ... --file -`).
/// The TUI still gets keys: crossterm reads them from /dev/tty when stdin
/// isn't a terminal.
pub fn spawn_stdin(tx: mpsc::Sender<SourceEvent>) -> (SourceInfo, tokio::task::JoinHandle<()>) {
    let id = "stdin".to_string();
    let info = SourceInfo {
        id: id.clone(),
        kind: "stdin".into(),
        status: SourceStatus::Starting,
    };
    let handle = tokio::spawn(async move {
        let _ = tx
            .send(SourceEvent::Status {
                source: id.clone(),
                status: SourceStatus::Running,
            })
            .await;
        forward_lines(tokio::io::stdin(), &id, tx.clone(), false).await;
        let _ = tx
            .send(SourceEvent::Status {
                source: id,
                status: SourceStatus::Stopped,
            })
            .await;
    });
    (info, handle)
}

async fn run_file_tail(path: &str, source_id: &str, tx: mpsc::Sender<SourceEvent>) -> Result<()> {
    let result = Command::new("tail")
        .args(["-f", "-n", "+1", "--", path])
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
        forward_lines(stdout, source_id, tx.clone(), false).await;
    }

    let _ = tx.send(SourceEvent::Status {
        source: source_id.to_string(),
        status: SourceStatus::Stopped,
    }).await;

    let _ = child.wait().await;
    Ok(())
}

#[cfg(test)]
mod security_tests {
    use super::*;

    async fn collect(input: &'static [u8]) -> Vec<String> {
        let (tx, mut rx) = mpsc::channel(64);
        forward_lines(input, "t", tx, false).await;
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let SourceEvent::Log { line, .. } = ev {
                out.push(line);
            }
        }
        out
    }

    /// A single invalid UTF-8 byte in a log must not end the stream: otherwise
    /// anyone who can write one byte into a log can silence the monitor.
    #[tokio::test]
    async fn invalid_utf8_does_not_stop_the_stream() {
        let lines = collect(b"before\n\xff\xfe broken\nafter\n").await;
        assert_eq!(lines.len(), 3, "got {:?}", lines);
        assert_eq!(lines[2], "after");
    }

    /// A line with no newline must not be buffered without bound.
    #[tokio::test]
    async fn very_long_line_is_capped() {
        static BIG: once_cell::sync::Lazy<Vec<u8>> = once_cell::sync::Lazy::new(|| {
            let mut v = vec![b'a'; 4 * 1024 * 1024];
            v.extend_from_slice(b"\nnext\n");
            v
        });
        let (tx, mut rx) = mpsc::channel(64);
        forward_lines(&BIG[..], "t", tx, false).await;
        let mut lines = Vec::new();
        while let Ok(SourceEvent::Log { line, .. }) = rx.try_recv() {
            lines.push(line);
        }
        assert_eq!(lines.len(), 2);
        assert!(lines[0].len() <= 64 * 1024 + 16, "line kept {} bytes", lines[0].len());
        assert_eq!(lines[1], "next");
    }

    #[test]
    fn curl_config_carries_header_and_cannot_add_lines() {
        assert_eq!(
            curl_config("Authorization: Bearer abc.def"),
            "header = \"Authorization: Bearer abc.def\"\n"
        );
        // A crafted token can't close the quote or start a new directive.
        let cfg = curl_config("Authorization: Bearer x\"\nurl = \"http://evil\\");
        assert_eq!(cfg.lines().count(), 1, "{}", cfg);
        assert_eq!(cfg, "header = \"Authorization: Bearer x\\\"url = \\\"http://evil\\\\\"\n");
    }

    /// Bearer tokens must never be on curl's command line, where any local
    /// user can read them from `ps` or /proc/<pid>/cmdline; yet curl must still
    /// send them. Checks the live process, so it fails however the token gets
    /// into argv. Skipped without curl.
    #[tokio::test]
    async fn curl_token_reaches_server_but_not_argv() {
        use tokio::io::AsyncReadExt;
        if std::process::Command::new("curl").arg("--version").output().is_err() {
            eprintln!("curl not installed; skipping");
            return;
        }
        let token = "probe-token-7f3a";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/probe", listener.local_addr().unwrap());
        let (got_req_tx, got_req_rx) = tokio::sync::oneshot::channel::<String>();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut req = vec![0u8; 4096];
            let n = sock.read(&mut req).await.unwrap();
            let _ = got_req_tx.send(String::from_utf8_lossy(&req[..n]).into_owned());
            // Hold the response so curl is still running while argv is read.
            let _ = reply_rx.await;
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").await;
        });

        let mut cmd = curl_command(&["-sf", "--noproxy", "*"], &url);
        cmd.stdout(std::process::Stdio::piped());
        let child = spawn_curl(cmd, &format!("Authorization: Bearer {}", token)).await.unwrap();

        let request = got_req_rx.await.unwrap();
        assert!(request.contains(&format!("Authorization: Bearer {}", token)), "{}", request);

        if let Some(pid) = child.id() {
            if let Ok(cmdline) = std::fs::read(format!("/proc/{}/cmdline", pid)) {
                let argv = String::from_utf8_lossy(&cmdline).replace('\0', " ");
                assert!(argv.contains("curl"), "unexpected process: {}", argv);
                assert!(!argv.contains(token), "token in argv: {}", argv);
            }
            if let Ok(environ) = std::fs::read(format!("/proc/{}/environ", pid)) {
                assert!(!String::from_utf8_lossy(&environ).contains(token), "token in environment");
            }
        }
        let _ = reply_tx.send(());
        assert!(child.wait_with_output().await.unwrap().status.success());
    }

    #[test]
    fn azure_url_parts_are_validated() {
        assert!(is_safe_url_segment("my-app_1"));
        assert!(is_safe_url_segment("rg-prod(eu)"));
        for bad in ["", "..", "a/b", "a?x=1", "a#f", "a@evil.com", "a%2f", "a b", "-\n"] {
            assert!(!is_safe_url_segment(bad), "{:?}", bad);
        }
        assert!(is_safe_resource_id(
            "/subscriptions/0000/resourceGroups/rg/providers/Microsoft.App/managedEnvironments/env"
        ));
        for bad in [
            "@evil.com/x",
            "/subscriptions/../../x",
            "/subscriptions/a?b",
            "//evil.com/subscriptions/x",
            "/subscriptions/a/@evil.com",
        ] {
            assert!(!is_safe_resource_id(bad), "{:?}", bad);
        }
        assert!(is_container_apps_domain("happy-sky-123.eastus.azurecontainerapps.io"));
        for bad in [
            "evil.com",
            "azurecontainerapps.io.evil.com",
            "evil.com/.azurecontainerapps.io",
            "evil.com#.azurecontainerapps.io",
            "a@evil.com.azurecontainerapps.io",
            "x..azurecontainerapps.io",
        ] {
            assert!(!is_container_apps_domain(bad), "{:?}", bad);
        }
    }

    #[test]
    fn kube_targets_parse_and_build_safe_args() {
        let t = KubeTarget::parse("api-7c9f").unwrap();
        assert_eq!(t.id(), "kube/api-7c9f");
        assert_eq!(
            t.kubectl_args(Follow::Yes),
            ["logs", "--follow", "--tail=100", "--all-containers=true", "--prefix=true", "--", "api-7c9f"]
        );
        let t = KubeTarget::parse("prod:deploy/api").unwrap();
        assert_eq!(t.id(), "kube/prod/deploy/api");
        assert_eq!(
            t.kubectl_args(Follow::No),
            ["logs", "--all-containers=true", "--prefix=true", "--namespace=prod", "--", "deploy/api"]
        );
        let t = KubeTarget::parse("prod:app=api,tier!=db").unwrap();
        assert_eq!(t.id(), "kube/prod/[app=api,tier!=db]");
        let args = t.kubectl_args(Follow::Yes);
        assert!(args.contains(&"--selector=app=api,tier!=db".to_string()));
        assert!(!args.contains(&"--".to_string()));
        for bad in [
            "",
            "--all-namespaces",
            "-n",
            "ns:-x",
            "a b",
            "deploy/",
            "/api",
            "pod;rm",
            "ns with space:api",
            "app=$(id)",
            "-l=app=x",
        ] {
            assert!(KubeTarget::parse(bad).is_err(), "{:?} should be rejected", bad);
        }
    }

    #[test]
    fn kube_prefix_is_stripped() {
        assert_eq!(strip_kube_prefix("[pod/api-7c9f-x2k4/api] GET /health 200".into()), "GET /health 200");
        assert_eq!(strip_kube_prefix("[INFO] not a kube prefix".into()), "[INFO] not a kube prefix");
        assert_eq!(strip_kube_prefix("[pod/unterminated".into()), "[pod/unterminated");
    }
}
