//! herdr integration — liveness hints ONLY. The link speaks the
//! socket API (newline-delimited JSON over `$HERDR_SOCKET_PATH`, protocol
//! 19): `session.snapshot` to learn which panes exist and what their agents
//! are doing, plus a held `events.subscribe` connection for pane lifecycle
//! and per-locator agent-status changes. Task state NEVER comes from here —
//! the contract file is the sole authority; these hints only annotate
//! locators (live status, dead-pane marks) and power `[enter]` focus. No
//! herdr, a dead socket, a mid-session restart: all degrade to "no hints",
//! never to a crash or a stale claim.
//!
//! Two envelope families arrive on the subscription (verified against a
//! live 0.8 daemon): global lifecycle events use underscore names
//! (`pane_closed` with top-level `data.pane_id`; `pane_created` /
//! `pane_updated` with the identity nested at `data.pane.pane_id`), while
//! parameterized per-pane subscriptions emit dotted names
//! (`pane.agent_status_changed` with top-level `pane_id` and
//! `agent_status`). The daemon also REPLAYS retained events per type from
//! its own sequence start on subscribe, so the stream is only a delta
//! after a snapshot taken once the replay burst has drained.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Hints {
    /// Subscription is acknowledged and the post-subscribe snapshot seeded.
    pub connected: bool,
    /// pane_id → agent_status. herdr's `AgentStatus` is a closed enum:
    /// `idle | working | blocked | done | unknown` — a pane with no
    /// detected agent reports "unknown" (verified against a live
    /// snapshot; the field is present on every pane). A missing key while
    /// `connected` means the pane is GONE — the one fact worth shouting.
    pub pane_status: HashMap<String, String>,
}

impl Hints {
    /// Status for a contract locator pane: `None` = no basis for a claim
    /// (not connected), `Some(None)` = pane is gone, `Some(Some(s))` = live.
    pub fn pane(&self, pane_id: &str) -> Option<Option<&str>> {
        if !self.connected {
            return None;
        }
        Some(self.pane_status.get(pane_id).map(String::as_str))
    }
}

pub struct Link {
    state: Arc<Mutex<Hints>>,
    /// Desired agent-status subscriptions (locator panes) + generation; the
    /// worker re-subscribes when the generation moves.
    watch: Arc<Mutex<(Vec<String>, u64)>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Link {
    /// Detect herdr from the environment. `None` (no socket path in the
    /// environment) is the normal non-herdr case and disables hints and
    /// focus — nothing else. A path that does not exist YET is not a
    /// reason to opt out: the daemon may be restarting, and the worker's
    /// retry loop will find the socket when it reappears.
    pub fn start() -> Option<Link> {
        let sock = std::env::var("HERDR_SOCKET_PATH").ok().filter(|s| !s.is_empty())?;
        Some(Link::start_at(sock))
    }

    fn start_at(sock: String) -> Link {
        let state = Arc::new(Mutex::new(Hints::default()));
        let watch = Arc::new(Mutex::new((Vec::new(), 0u64)));
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let state = Arc::clone(&state);
            let watch = Arc::clone(&watch);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || worker(&sock, &state, &watch, &stop))
        };
        Link { state, watch, stop, handle: Some(handle) }
    }

    pub fn hints(&self) -> Hints {
        self.state.lock().map(|h| h.clone()).unwrap_or_default()
    }

    /// Declare which panes (contract locators) deserve per-pane
    /// subscriptions. Idempotent; bumps the generation only on change.
    pub fn set_watch(&self, mut panes: Vec<String>) {
        panes.sort();
        panes.dedup();
        if let Ok(mut w) = self.watch.lock() {
            if w.0 != panes {
                w.0 = panes;
                w.1 += 1;
            }
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // The worker notices within one read-timeout tick (1s) or one
        // retry-sleep step (100ms); join is bounded, not five seconds.
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// ── worker: subscribe, snapshot, keep the map fresh ─────────────────

#[cfg(unix)]
fn worker(
    sock: &str,
    state: &Arc<Mutex<Hints>>,
    watch: &Arc<Mutex<(Vec<String>, u64)>>,
    stop: &Arc<AtomicBool>,
) {
    let mut failures: u32 = 0;
    while !stop.load(Ordering::Relaxed) {
        let r = session(sock, state, watch, stop);
        // Session over, for whatever reason: every claim in the map is now
        // unverifiable. Drop them all — disconnected must never keep
        // asserting GONE (or alive) from a dead stream.
        if let Ok(mut h) = state.lock() {
            h.connected = false;
            h.pane_status.clear();
        }
        if r.is_err() {
            // exponential backoff, 5s → 60s cap: a dead daemon should not
            // cost hundreds of connect attempts per hour forever
            failures = failures.saturating_add(1);
            let wait_s = (5u64 << (failures - 1).min(4)).min(60);
            for _ in 0..(wait_s * 10) {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        } else {
            failures = 0;
            // Ok(()) means the watch set changed: resubscribe immediately.
        }
    }
}

#[cfg(not(unix))]
fn worker(
    _: &str,
    _: &Arc<Mutex<Hints>>,
    _: &Arc<Mutex<(Vec<String>, u64)>>,
    _: &Arc<AtomicBool>,
) {
}

/// Longest stream record we accept before declaring the peer wedged; a
/// hostile or broken daemon must not grow our buffer without bound.
#[cfg(unix)]
const MAX_LINE: usize = 1 << 20;

/// One `session.snapshot` round-trip → pane map. Strict on identity: a
/// snapshot pane without a string `pane_id` fails the WHOLE snapshot
/// (skipping it would silently turn a live pane into GONE).
#[cfg(unix)]
fn fetch_snapshot(sock: &str) -> Result<HashMap<String, String>, ()> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let mut conn = UnixStream::connect(sock).map_err(|_| ())?;
    conn.set_read_timeout(Some(Duration::from_secs(5))).ok();
    conn.write_all(b"{\"id\":\"dagr-snap\",\"method\":\"session.snapshot\",\"params\":{}}\n")
        .map_err(|_| ())?;
    let mut line = String::new();
    BufReader::new(&conn).read_line(&mut line).map_err(|_| ())?;
    let v: serde_json::Value = serde_json::from_str(&line).map_err(|_| ())?;
    if v.get("error").is_some() {
        return Err(());
    }
    let panes = v
        .pointer("/result/snapshot/panes")
        .and_then(|p| p.as_array())
        .ok_or(())?;
    let mut map = HashMap::new();
    for p in panes {
        let id = p.get("pane_id").and_then(|x| x.as_str()).ok_or(())?;
        let st = p
            .get("agent_status")
            .and_then(|x| x.as_str())
            .unwrap_or("unknown");
        map.insert(id.to_string(), st.to_string());
    }
    Ok(map)
}

#[cfg(unix)]
fn session(
    sock: &str,
    state: &Arc<Mutex<Hints>>,
    watch: &Arc<Mutex<(Vec<String>, u64)>>,
    stop: &Arc<AtomicBool>,
) -> Result<(), ()> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    // The watch list and its generation are read under ONE lock: sampling
    // them separately lets a set_watch land in between, subscribing the
    // new panes under the old generation and forcing a spurious rebuild.
    let (wanted, gen) = watch
        .lock()
        .map(|w| (w.0.clone(), w.1))
        .unwrap_or_default();

    // 1) pre-snapshot: which panes exist right now. Used only to filter
    // the per-pane subscriptions — subscribing to a nonexistent pane_id is
    // an API error that would kill the link, and a dead locator needs no
    // subscription (its absence from the snapshot IS the answer).
    let known = fetch_snapshot(sock)?;
    let panes: Vec<String> = wanted
        .into_iter()
        .filter(|p| known.contains_key(p))
        .collect();

    // 2) held subscription: pane lifecycle globally + agent status per
    // watched locator pane (the API requires a pane_id there).
    let mut subs = vec![
        serde_json::json!({"type": "pane.created"}),
        serde_json::json!({"type": "pane.closed"}),
        serde_json::json!({"type": "pane.exited"}),
        serde_json::json!({"type": "pane.agent_detected"}),
    ];
    for p in &panes {
        subs.push(serde_json::json!({"type": "pane.agent_status_changed", "pane_id": p}));
    }
    let req = serde_json::json!({
        "id": "dagr-sub", "method": "events.subscribe",
        "params": {"subscriptions": subs}
    });
    let mut conn = UnixStream::connect(sock).map_err(|_| ())?;
    conn.set_read_timeout(Some(Duration::from_secs(1))).ok();
    conn.write_all((req.to_string() + "\n").as_bytes()).map_err(|_| ())?;
    let mut reader = BufReader::new(conn);

    // 3) wait for the acknowledgement — nothing is claimed before
    // `subscription_started` arrives for our request id. Replayed events
    // showing up early are discarded: they predate the seed snapshot below.
    let mut line = String::new();
    let acked = 'ack: {
        for _ in 0..10 {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => return Err(()),
                Ok(_) if line.len() > MAX_LINE => return Err(()),
                Ok(_) => {
                    let v = parse_record(&line)?;
                    if v.get("id").and_then(|i| i.as_str()) == Some("dagr-sub") {
                        let ok = v.pointer("/result/type").and_then(|t| t.as_str())
                            == Some("subscription_started");
                        if !ok {
                            return Err(());
                        }
                        break 'ack true;
                    }
                    // pre-ack replay event: drop
                }
                Err(e) if timeoutish(&e) => {}
                Err(_) => return Err(()),
            }
        }
        false
    };
    if !acked {
        return Err(());
    }

    // 4) drain the replay burst. Retained events replay per type from the
    // daemon's own sequence start — cross-type order is NOT history (a
    // stale `pane_agent_detected` can replay after the pane's `closed`).
    // Everything here is discarded; the snapshot below reflects it all.
    reader.get_ref().set_read_timeout(Some(Duration::from_millis(250))).ok();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return Err(()),
            Ok(_) if line.len() > MAX_LINE => return Err(()),
            Ok(_) => {
                parse_record(&line)?;
            }
            Err(e) if timeoutish(&e) => break,
            Err(_) => return Err(()),
        }
    }
    reader.get_ref().set_read_timeout(Some(Duration::from_secs(1))).ok();

    // 5) seed from a FRESH snapshot, taken after the replay drained: from
    // here the stream is a genuine delta. Only now do hints go live.
    let map = fetch_snapshot(sock)?;
    if let Ok(mut h) = state.lock() {
        h.pane_status = map;
        h.connected = true;
    }

    // 6) event loop. A malformed record tears the session down (Err) —
    // an unparseable stream cannot back an absence claim.
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return Err(()), // server closed
            Ok(_) if line.len() > MAX_LINE => return Err(()),
            Ok(_) => {
                let v = parse_record(&line)?;
                if let Some(ev) = v.get("event").and_then(|e| e.as_str()) {
                    apply_event(state, ev, v.get("data"));
                }
                // a run-file reload may have changed the watch set
                if watch.lock().map(|w| w.1).unwrap_or(gen) != gen {
                    return Ok(());
                }
            }
            Err(e) if timeoutish(&e) => {
                if watch.lock().map(|w| w.1).unwrap_or(gen) != gen {
                    return Ok(());
                }
            }
            Err(_) => return Err(()),
        }
    }
}

#[cfg(unix)]
fn timeoutish(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// Strict stream-record parse: a line must be a JSON object shaped like a
/// response (`id`), an error, or an event (`event` + object `data`).
/// Anything else is a protocol breach, not something to skim past.
#[cfg(unix)]
fn parse_record(line: &str) -> Result<serde_json::Value, ()> {
    let v: serde_json::Value = serde_json::from_str(line).map_err(|_| ())?;
    if !v.is_object() || v.get("error").is_some() {
        return Err(());
    }
    let is_event = v.get("event").map(|e| e.is_string()).unwrap_or(false)
        && v.get("data").map(|d| d.is_object()).unwrap_or(false);
    let is_response = v.get("id").is_some();
    if is_event || is_response {
        Ok(v)
    } else {
        Err(())
    }
}

/// Pane identity: top-level `pane_id` (global lifecycle + parameterized
/// events) or nested `pane.pane_id` (`pane_created`/`pane_updated` carry a
/// full PaneInfo).
#[cfg(unix)]
fn evt_pane_id(data: Option<&serde_json::Value>) -> Option<String> {
    let d = data?;
    d.get("pane_id")
        .and_then(|p| p.as_str())
        .or_else(|| d.pointer("/pane/pane_id").and_then(|p| p.as_str()))
        .map(String::from)
}

/// Agent status, wherever the envelope put it. `None` when the event
/// genuinely carries none — never invented.
#[cfg(unix)]
fn evt_status(data: Option<&serde_json::Value>) -> Option<String> {
    let d = data?;
    d.get("agent_status")
        .and_then(|s| s.as_str())
        .or_else(|| d.pointer("/pane/agent_status").and_then(|s| s.as_str()))
        .map(String::from)
}

#[cfg(unix)]
fn apply_event(state: &Arc<Mutex<Hints>>, event: &str, data: Option<&serde_json::Value>) {
    let Some(pane) = evt_pane_id(data) else { return };
    let Ok(mut h) = state.lock() else { return };
    match event {
        // Only pane_closed is the pane going away. pane_exited is the
        // pane's PROCESS exiting — the pane may persist and stay
        // focusable, so it becomes a status fact, never a GONE claim.
        "pane_closed" | "pane.closed" => {
            h.pane_status.remove(&pane);
        }
        "pane_exited" | "pane.exited" => {
            if h.pane_status.contains_key(&pane) {
                h.pane_status.insert(pane, "unknown".into());
            }
        }
        "pane_created" | "pane_updated" | "pane.created" | "pane.updated" => {
            let st = evt_status(data).unwrap_or_else(|| "unknown".into());
            h.pane_status.insert(pane, st);
        }
        // Parameterized subscriptions arrive dotted; the global variant of
        // the same event is underscored. Both carry top-level status.
        "pane.agent_status_changed" | "pane_agent_status_changed" => {
            if let Some(st) = evt_status(data) {
                h.pane_status.insert(pane, st);
            }
        }
        // Detection is not membership and carries no current status:
        // `released: true` means the agent is gone — a fresh snapshot of
        // an agentless pane reports "unknown", so that is what we record
        // (final_status describes the departed agent's last state, not
        // the pane's current one). A fresh detection is followed by real
        // status events. Unknown panes are never invented from
        // replay-prone detections.
        "pane_agent_detected" | "pane.agent_detected" => {
            if h.pane_status.contains_key(&pane)
                && data
                    .and_then(|d| d.get("released"))
                    .and_then(|r| r.as_bool())
                    .unwrap_or(false)
            {
                h.pane_status.insert(pane, "unknown".into());
            }
        }
        _ => {}
    }
}

// ── focus: the one action M2 ships ──────────────────────────────────
//
// Protocol 19 exposes `pane.focus` (PaneTarget) and `agent.focus`
// (AgentTarget) directly on the socket — one short-lived round trip, no
// subprocess, no zoom-cycle side effects on the user's layout, and the
// producer-controlled locator travels as a typed JSON string, so there is
// no argv/flag-injection surface either.

#[cfg(unix)]
fn socket_request(method: &str, params: serde_json::Value) -> Result<(), String> {
    let sock = std::env::var("HERDR_SOCKET_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "not inside herdr".to_string())?;
    socket_request_at(&sock, method, params)
}

#[cfg(unix)]
fn socket_request_at(
    sock: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let req = serde_json::json!({"id": "dagr-focus", "method": method, "params": params});
    let mut conn =
        UnixStream::connect(sock).map_err(|e| format!("herdr socket: {e}"))?;
    conn.set_read_timeout(Some(Duration::from_secs(3))).ok();
    conn.set_write_timeout(Some(Duration::from_secs(3))).ok();
    conn.write_all((req.to_string() + "\n").as_bytes())
        .map_err(|e| format!("herdr socket: {e}"))?;
    let mut line = String::new();
    BufReader::new(&conn)
        .read_line(&mut line)
        .map_err(|e| format!("herdr socket: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&line).map_err(|_| "herdr: malformed response".to_string())?;
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("request failed");
        return Err(format!("herdr: {msg}"));
    }
    Ok(())
}

#[cfg(not(unix))]
fn socket_request(_: &str, _: serde_json::Value) -> Result<(), String> {
    Err("herdr link requires unix sockets".into())
}

/// Focus a locator pane by id (`pane.focus`).
pub fn focus_pane(pane_id: &str) -> Result<(), String> {
    socket_request("pane.focus", serde_json::json!({"pane_id": pane_id}))
}

/// Focus a locator agent by name (`agent.focus`).
pub fn focus_agent(name: &str) -> Result<(), String> {
    socket_request("agent.focus", serde_json::json!({"target": name}))
}

/// Queue an operator message to an orchestrator pane/agent. Herdr owns the
/// input queue; dagr neither waits for nor interprets the response.
#[cfg(unix)]
pub fn prompt(target: &str, text: &str) -> Result<(), String> {
    socket_request(
        "agent.prompt",
        serde_json::json!({"target": target, "text": text}),
    )
}

/// Herdr uses a CLI transport on platforms without Unix-domain sockets.
#[cfg(any(not(unix), test))]
fn prompt_cli_args<'a>(target: &'a str, text: &'a str) -> [&'a str; 4] {
    ["agent", "prompt", target, text]
}

#[cfg(not(unix))]
pub fn prompt(target: &str, text: &str) -> Result<(), String> {
    let output = std::process::Command::new("herdr")
        .args(prompt_cli_args(target, text))
        .output()
        .map_err(|e| format!("cannot launch herdr: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if detail.is_empty() { "herdr prompt failed".into() } else { detail })
    }
}

// ── tests: envelope fixtures from a live protocol-19 daemon ─────────

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn hints_with(pairs: &[(&str, &str)]) -> Arc<Mutex<Hints>> {
        let mut h = Hints { connected: true, pane_status: HashMap::new() };
        for (k, v) in pairs {
            h.pane_status.insert((*k).into(), (*v).into());
        }
        Arc::new(Mutex::new(h))
    }

    fn apply(state: &Arc<Mutex<Hints>>, line: &str) {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let ev = v.get("event").unwrap().as_str().unwrap().to_string();
        apply_event(state, &ev, v.get("data"));
    }

    fn status(state: &Arc<Mutex<Hints>>, pane: &str) -> Option<String> {
        state.lock().unwrap().pane_status.get(pane).cloned()
    }

    // Captured live: parameterized subscription events arrive DOTTED with
    // top-level pane_id/agent_status.
    #[test]
    fn dotted_status_event_applies() {
        let st = hints_with(&[("wF:pK", "idle")]);
        apply(
            &st,
            r#"{"data":{"agent":"claude","agent_status":"working","pane_id":"wF:pK","workspace_id":"wF"},"event":"pane.agent_status_changed"}"#,
        );
        assert_eq!(status(&st, "wF:pK").as_deref(), Some("working"));
    }

    // Captured live: pane_created nests identity under data.pane.
    #[test]
    fn created_event_reads_nested_pane() {
        let st = hints_with(&[]);
        apply(
            &st,
            r#"{"data":{"pane":{"pane_id":"wF:pX","workspace_id":"wF","agent_status":null},"type":"pane_created"},"event":"pane_created"}"#,
        );
        assert_eq!(status(&st, "wF:pX").as_deref(), Some("unknown"));
    }

    // pane_exited is the process exiting, not the pane going away: it
    // must degrade the status, never manufacture a GONE claim.
    #[test]
    fn exited_event_degrades_status_but_keeps_pane() {
        let st = hints_with(&[("wF:pX", "working")]);
        apply(
            &st,
            r#"{"data":{"pane_id":"wF:pX","type":"pane_exited","workspace_id":"wF"},"event":"pane_exited"}"#,
        );
        assert_eq!(status(&st, "wF:pX").as_deref(), Some("unknown"));
    }

    // The honesty primitive: no claims of any kind while disconnected.
    #[test]
    fn hints_claim_nothing_while_disconnected() {
        let mut h = Hints::default();
        h.pane_status.insert("p".into(), "working".into());
        assert_eq!(h.pane("p"), None);
        assert_eq!(h.pane("missing"), None);
        h.connected = true;
        assert_eq!(h.pane("p"), Some(Some("working")));
        assert_eq!(h.pane("missing"), Some(None));
    }

    // Captured live: pane_closed carries top-level data.pane_id.
    #[test]
    fn closed_event_removes_pane() {
        let st = hints_with(&[("wF:pX", "working")]);
        apply(
            &st,
            r#"{"data":{"pane_id":"wF:pX","type":"pane_closed","workspace_id":"wF"},"event":"pane_closed"}"#,
        );
        assert_eq!(status(&st, "wF:pX"), None);
    }

    #[test]
    fn detection_never_invents_membership_or_status() {
        // unknown pane: replayed detection must not resurrect it
        let st = hints_with(&[("known", "working")]);
        apply(
            &st,
            r#"{"data":{"pane_id":"ghost","agent":"claude","released":false,"type":"pane_agent_detected"},"event":"pane_agent_detected"}"#,
        );
        assert_eq!(status(&st, "ghost"), None);
        // known pane, fresh detection (no current status in the event):
        // existing knowledge stands until a real status event
        apply(
            &st,
            r#"{"data":{"pane_id":"known","agent":"claude","released":false,"type":"pane_agent_detected"},"event":"pane_agent_detected"}"#,
        );
        assert_eq!(status(&st, "known").as_deref(), Some("working"));
        // release: the agent is gone — an agentless pane snapshots as
        // "unknown", so that is what the release records
        apply(
            &st,
            r#"{"data":{"pane_id":"known","agent":"claude","released":true,"final_status":"done","type":"pane_agent_detected"},"event":"pane_agent_detected"}"#,
        );
        assert_eq!(status(&st, "known").as_deref(), Some("unknown"));
    }

    #[test]
    fn status_event_without_status_is_ignored() {
        let st = hints_with(&[("p", "idle")]);
        apply(
            &st,
            r#"{"data":{"pane_id":"p"},"event":"pane.agent_status_changed"}"#,
        );
        assert_eq!(status(&st, "p").as_deref(), Some("idle"));
    }

    #[test]
    fn record_parsing_is_strict() {
        assert!(parse_record("not json\n").is_err());
        assert!(parse_record(r#"{"event":"x"}"#).is_err()); // no data
        assert!(parse_record(r#"{"error":{"code":1}}"#).is_err());
        assert!(parse_record(r#"[1,2]"#).is_err());
        assert!(parse_record(r#"{"id":"dagr-sub","result":{"type":"subscription_started"}}"#).is_ok());
        assert!(parse_record(r#"{"event":"pane_closed","data":{"pane_id":"p"}}"#).is_ok());
    }

    #[test]
    fn prompt_uses_typed_agent_prompt_request() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;

        let dir = std::env::temp_dir().join(format!(
            "dagr-prompt-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("herdr.sock");
        let listener = match UnixListener::bind(&sock) {
            Ok(listener) => listener,
            Err(e) => {
                eprintln!("SKIPPED prompt_uses_typed_agent_prompt_request: {e}");
                return;
            }
        };
        let server = std::thread::spawn(move || {
            let (conn, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(conn);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let req: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["method"], "agent.prompt");
            assert_eq!(req["params"]["target"], "wX:p1");
            assert_eq!(req["params"]["text"], "hello\nworld");
            reader
                .get_mut()
                .write_all(b"{\"id\":\"dagr-focus\",\"result\":{}}\n")
                .unwrap();
        });
        socket_request_at(
            sock.to_str().unwrap(),
            "agent.prompt",
            serde_json::json!({"target":"wX:p1","text":"hello\nworld"}),
        )
        .unwrap();
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn windows_prompt_cli_uses_the_documented_positional_shape() {
        assert_eq!(
            prompt_cli_args("wX:p1", "hello world"),
            ["agent", "prompt", "wX:p1", "hello world"]
        );
    }

    /// Full session against a fake daemon: ack gate, replay discard,
    /// post-drain seed, live delta — each of these is a failure mode
    /// this module actually shipped once and now guards against.
    #[test]
    fn session_against_fake_daemon() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;

        let dir = std::env::temp_dir().join(format!("dagr-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("herdr.sock");
        let _ = std::fs::remove_file(&sock);
        // Sandboxed CI/review environments sometimes deny AF_UNIX binds;
        // that is an environment limit, not a product signal — skip loudly.
        let listener = match UnixListener::bind(&sock) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("SKIPPED session_against_fake_daemon: cannot bind a unix socket here ({e})");
                return;
            }
        };

        let server = std::thread::spawn(move || {
            let snap = |conn: std::os::unix::net::UnixStream, panes: &str| {
                let mut r = BufReader::new(conn);
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                assert!(line.contains("session.snapshot"));
                let resp = format!(
                    "{{\"id\":\"dagr-snap\",\"result\":{{\"snapshot\":{{\"panes\":[{panes}]}}}}}}\n"
                );
                r.get_ref().write_all(resp.as_bytes()).unwrap();
            };
            // 1) pre-snapshot (filter basis)
            snap(listener.accept().unwrap().0, r#"{"pane_id":"wF:pA","agent_status":"idle"}"#);
            // 2) subscription: ack, then a stale replay pair, then (after
            //    the drain window) a live dotted status event
            let (sub, _) = listener.accept().unwrap();
            let mut r = BufReader::new(sub);
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            assert!(line.contains("events.subscribe"));
            assert!(line.contains("\"pane_id\":\"wF:pA\"")); // watched, known
            let mut w = r.get_ref();
            w.write_all(b"{\"id\":\"dagr-sub\",\"result\":{\"type\":\"subscription_started\"}}\n").unwrap();
            // replayed history: a pane that was created and closed long ago
            w.write_all(b"{\"event\":\"pane_created\",\"data\":{\"pane\":{\"pane_id\":\"wF:pOld\"},\"type\":\"pane_created\"}}\n").unwrap();
            w.write_all(b"{\"event\":\"pane_closed\",\"data\":{\"pane_id\":\"wF:pOld\",\"type\":\"pane_closed\"}}\n").unwrap();
            // 3) post-drain seed snapshot
            snap(listener.accept().unwrap().0, r#"{"pane_id":"wF:pA","agent_status":"idle"}"#);
            // 4) live delta on the held subscription
            std::thread::sleep(Duration::from_millis(600));
            let mut w = r.get_ref();
            w.write_all(b"{\"event\":\"pane.agent_status_changed\",\"data\":{\"pane_id\":\"wF:pA\",\"agent_status\":\"working\",\"agent\":\"claude\"}}\n").unwrap();
            std::thread::sleep(Duration::from_millis(400));
            // dropping the stream ends the session
        });

        let state = Arc::new(Mutex::new(Hints::default()));
        let watch = Arc::new(Mutex::new((vec!["wF:pA".to_string()], 1u64)));
        let stop = Arc::new(AtomicBool::new(false));
        let sock_s = sock.to_str().unwrap().to_string();
        let (st2, wa2, sp2) = (Arc::clone(&state), Arc::clone(&watch), Arc::clone(&stop));
        let client =
            std::thread::spawn(move || session(&sock_s, &st2, &wa2, &sp2));

        // connected goes true only after ack + drain + seed
        let mut connected_at = None;
        for i in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            if state.lock().unwrap().connected {
                connected_at = Some(i);
                break;
            }
        }
        assert!(connected_at.is_some(), "never connected");
        // replayed pOld must NOT be in the map; watched pane seeded
        {
            let h = state.lock().unwrap();
            assert!(!h.pane_status.contains_key("wF:pOld"));
            assert_eq!(h.pane_status.get("wF:pA").map(String::as_str), Some("idle"));
        }
        // the live dotted event lands
        let mut saw_working = false;
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            if state.lock().unwrap().pane_status.get("wF:pA").map(String::as_str)
                == Some("working")
            {
                saw_working = true;
                break;
            }
        }
        assert!(saw_working, "live dotted status event was not applied");

        // server hangs up → session ends as an error (worker would retry)
        server.join().unwrap();
        assert!(client.join().unwrap().is_err());
        let _ = std::fs::remove_file(&sock);
    }

    /// A subscription peer that never acknowledges must never go live.
    #[test]
    fn no_ack_never_connects() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;

        let dir = std::env::temp_dir().join(format!("dagr-test-noack-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("herdr.sock");
        let _ = std::fs::remove_file(&sock);
        // Same environment guard as session_against_fake_daemon.
        let listener = match UnixListener::bind(&sock) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("SKIPPED no_ack_never_connects: cannot bind a unix socket here ({e})");
                return;
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stop_srv = Arc::clone(&stop);

        let server = std::thread::spawn(move || {
            // pre-snapshot succeeds
            let (conn, _) = listener.accept().unwrap();
            let mut r = BufReader::new(conn);
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            r.get_ref()
                .write_all(b"{\"id\":\"dagr-snap\",\"result\":{\"snapshot\":{\"panes\":[]}}}\n")
                .unwrap();
            // subscription accepted, request read, then… silence
            let (sub, _) = listener.accept().unwrap();
            let mut r = BufReader::new(sub);
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            while !stop_srv.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
            }
        });

        let state = Arc::new(Mutex::new(Hints::default()));
        let watch = Arc::new(Mutex::new((Vec::new(), 0u64)));
        let sock_s = sock.to_str().unwrap().to_string();
        let (st2, sp2) = (Arc::clone(&state), Arc::clone(&stop));
        let client = std::thread::spawn(move || session(&sock_s, &st2, &watch, &sp2));

        std::thread::sleep(Duration::from_millis(1500));
        assert!(!state.lock().unwrap().connected, "went live without an ack");
        stop.store(true, Ordering::Relaxed);
        let _ = client.join().unwrap();
        server.join().unwrap();
        let _ = std::fs::remove_file(&sock);
    }
}
