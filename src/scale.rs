//! Small, explicit safety bounds for untrusted run documents. These are
//! admission checks only: no alternate parser, cache, or scheduler lives here.

use crate::contract::Doc;
use std::io::Read;

pub const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_GRAPH_ITEMS: usize = 4096;
pub const MAX_EVENTS: usize = 32_768;
pub const MAX_FRAME_WIDTH: usize = 4096;

pub fn read_limited(path: &str) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    if file.metadata().map_err(|e| format!("cannot inspect {path}: {e}"))?.len()
        > MAX_SOURCE_BYTES
    {
        return Err(format!(
            "{path} exceeds the {} byte source limit",
            MAX_SOURCE_BYTES
        ));
    }
    let mut raw = String::new();
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_string(&mut raw)
        .map_err(|e| format!("cannot read {path} as UTF-8: {e}"))?;
    if raw.len() as u64 > MAX_SOURCE_BYTES {
        return Err(format!(
            "{path} exceeds the {} byte source limit",
            MAX_SOURCE_BYTES
        ));
    }
    Ok(raw)
}

pub fn enforce_document(doc: &Doc) -> Result<(), String> {
    let graph_items = doc.projects.len().saturating_add(
        doc.tasks.as_deref().unwrap_or(&[]).iter().fold(0usize, |total, task| {
            total
                .saturating_add(1)
                .saturating_add(task.attempts.len())
                .saturating_add(task.policy.as_ref().map_or(0, |p| p.futures.len()))
        }),
    );
    if graph_items > MAX_GRAPH_ITEMS {
        return Err(format!(
            "document has {graph_items} project/task/attempt/future items; limit is {MAX_GRAPH_ITEMS}"
        ));
    }
    if doc.events.len() > MAX_EVENTS {
        return Err(format!(
            "document has {} events; limit is {MAX_EVENTS}",
            doc.events.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_and_event_counts_are_bounded() {
        let mut graph: Doc = serde_json::from_str(r#"{"tasks":[]}"#).unwrap();
        graph.tasks = Some(
            (0..=MAX_GRAPH_ITEMS)
                .map(|_| serde_json::from_str(r#"{"attempts":[]}"#).unwrap())
                .collect(),
        );
        assert!(enforce_document(&graph).unwrap_err().contains("limit"));

        let mut events: Doc = serde_json::from_str(r#"{"events":[]}"#).unwrap();
        events.events = (0..=MAX_EVENTS)
            .map(|_| serde_json::from_str("{}").unwrap())
            .collect();
        assert!(enforce_document(&events).unwrap_err().contains("events"));
    }
}
