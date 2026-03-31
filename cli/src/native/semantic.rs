use std::collections::{HashMap, HashSet};

use serde_json::Value;
use similar::TextDiff;

use super::cdp::client::CdpClient;
use super::cdp::types::{AXNode, GetFullAXTreeResult};
use super::element::{resolve_ax_session, RefMap};
use super::snapshot::{extract_ax_string, extract_ax_string_opt, CONTENT_ROLES, INTERACTIVE_ROLES};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub struct SemanticFindOptions {
    pub query: String,
    pub role_hint: Option<String>,
    pub within_ref: Option<String>,
    pub wait_ms: Option<u64>,
    pub top_k: usize,
    pub threshold: f32,
}

#[derive(Clone, Debug)]
pub struct SemanticMatch {
    pub ref_id: String,
    pub role: String,
    pub name: String,
    pub score: f32,
    pub backend_node_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

struct SemanticCandidate {
    role: String,
    name: String,
    value_text: Option<String>,
    backend_node_id: Option<i64>,
    is_interactive: bool,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn semantic_find(
    client: &CdpClient,
    session_id: &str,
    options: &SemanticFindOptions,
    ref_map: &mut RefMap,
    frame_id: Option<&str>,
    iframe_sessions: &HashMap<String, String>,
) -> Result<Vec<SemanticMatch>, String> {
    // Resolve --within scope to a set of backend_node_ids
    let within_ids = if let Some(ref within_ref) = options.within_ref {
        let ref_id = within_ref.trim_start_matches('@');
        let entry = ref_map
            .get(ref_id)
            .ok_or_else(|| format!("Unknown ref '{}' for --within", within_ref))?;
        let bid = entry
            .backend_node_id
            .ok_or_else(|| format!("Ref '{}' has no backend_node_id", within_ref))?;

        let describe: Value = client
            .send_command(
                "DOM.describeNode",
                Some(serde_json::json!({ "backendNodeId": bid, "depth": -1 })),
                Some(session_id),
            )
            .await?;

        let mut ids = HashSet::new();
        if let Some(node) = describe.get("node") {
            collect_backend_node_ids(node, &mut ids);
        }
        Some(ids)
    } else {
        None
    };

    let query_lower = options.query.to_lowercase();
    let role_hint = options.role_hint.as_deref();

    // Attempt with optional retry loop
    let deadline = options
        .wait_ms
        .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));

    loop {
        // Enable required domains
        let _ = client
            .send_command_no_params("DOM.enable", Some(session_id))
            .await;
        let _ = client
            .send_command_no_params("Accessibility.enable", Some(session_id))
            .await;

        // Fetch AX tree
        let (ax_params, effective_session_id) =
            resolve_ax_session(frame_id, session_id, iframe_sessions);
        if effective_session_id != session_id {
            let _ = client
                .send_command_no_params("DOM.enable", Some(effective_session_id))
                .await;
            let _ = client
                .send_command_no_params("Accessibility.enable", Some(effective_session_id))
                .await;
        }
        let ax_tree: GetFullAXTreeResult = client
            .send_command_typed(
                "Accessibility.getFullAXTree",
                &ax_params,
                Some(effective_session_id),
            )
            .await?;

        // Harvest and score
        let candidates = harvest_candidates(&ax_tree.nodes, within_ids.as_ref());
        let mut scored: Vec<(SemanticCandidate, f32)> = candidates
            .into_iter()
            .map(|c| {
                let s = score_candidate(&c, &options.query, &query_lower, role_hint);
                (c, s)
            })
            .filter(|(_, s)| *s >= options.threshold)
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(options.top_k);

        if !scored.is_empty() {
            // Populate ref_map with matches
            let mut matches = Vec::with_capacity(scored.len());
            let mut next_ref = ref_map.next_ref_num();

            for (candidate, score) in scored {
                let ref_id = format!("e{}", next_ref);
                next_ref += 1;

                ref_map.add_with_frame(
                    ref_id.clone(),
                    candidate.backend_node_id,
                    &candidate.role,
                    &candidate.name,
                    None,
                    frame_id,
                );

                matches.push(SemanticMatch {
                    ref_id,
                    role: candidate.role,
                    name: candidate.name,
                    score,
                    backend_node_id: candidate.backend_node_id,
                });
            }

            ref_map.set_next_ref_num(next_ref);
            return Ok(matches);
        }

        // Check deadline for retry
        match deadline {
            Some(d) if std::time::Instant::now() < d => {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            _ => break,
        }
    }

    Ok(Vec::new())
}

// ---------------------------------------------------------------------------
// Candidate harvesting
// ---------------------------------------------------------------------------

fn harvest_candidates(
    nodes: &[AXNode],
    within_ids: Option<&HashSet<i64>>,
) -> Vec<SemanticCandidate> {
    let mut candidates = Vec::new();

    for node in nodes {
        if node.ignored.unwrap_or(false) {
            continue;
        }

        let role = extract_ax_string(&node.role);
        if role.is_empty() {
            continue;
        }

        let name = extract_ax_string(&node.name);
        let backend_node_id = node.backend_d_o_m_node_id;

        // Filter by --within scope
        if let Some(ids) = within_ids {
            match backend_node_id {
                Some(bid) if ids.contains(&bid) => {}
                _ => continue,
            }
        }

        let is_interactive = INTERACTIVE_ROLES.contains(&role.as_str());
        let is_content = CONTENT_ROLES.contains(&role.as_str());

        // Same logic as snapshot ref assignment: interactive always, content if named
        if !is_interactive && !(is_content && !name.is_empty()) {
            continue;
        }

        let value_text = extract_ax_string_opt(&node.value);

        candidates.push(SemanticCandidate {
            role,
            name,
            value_text,
            backend_node_id,
            is_interactive,
        });
    }

    candidates
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

fn score_candidate(
    candidate: &SemanticCandidate,
    _query: &str,
    query_lower: &str,
    role_hint: Option<&str>,
) -> f32 {
    let name_lower = candidate.name.to_lowercase();
    let mut score: f32 = 0.0;

    if name_lower.is_empty() && candidate.value_text.is_none() {
        return 0.0;
    }

    // === Primary signals (take max) ===

    if !name_lower.is_empty() {
        if name_lower == query_lower {
            // Exact match (0.95 base leaves room for bonus signals)
            score = f32::max(score, 0.95);
        } else if name_lower.starts_with(query_lower) || query_lower.starts_with(&name_lower) {
            // Prefix overlap
            let shorter = name_lower.len().min(query_lower.len()) as f32;
            let longer = name_lower.len().max(query_lower.len()) as f32;
            let overlap = shorter / longer;
            score = f32::max(score, 0.6 + overlap * 0.3);
        } else if name_lower.contains(query_lower) {
            // Name contains query
            let ratio = query_lower.len() as f32 / name_lower.len() as f32;
            score = f32::max(score, 0.4 + ratio * 0.3);
        } else if query_lower.contains(&name_lower) && name_lower.len() >= 3 {
            // Query contains name
            let ratio = name_lower.len() as f32 / query_lower.len() as f32;
            score = f32::max(score, 0.35 + ratio * 0.3);
        } else {
            // Fuzzy match via similar crate
            let ratio = fuzzy_ratio(&name_lower, query_lower);
            if ratio > 0.6 {
                score = f32::max(score, ratio * 0.7);
            }
        }
    }

    // === Secondary signals (additive bonuses) ===

    // Value text match (for inputs with placeholder/current value)
    if let Some(ref vt) = candidate.value_text {
        let vt_lower = vt.to_lowercase();
        if vt_lower == query_lower {
            score = f32::max(score, 0.8);
        } else if vt_lower.contains(query_lower) {
            score += 0.15;
        }
    }

    // Role hint bonus/penalty
    if let Some(rh) = role_hint {
        if candidate.role.eq_ignore_ascii_case(rh) {
            score += 0.15;
        } else {
            score -= 0.05;
        }
    }

    // Interactive element boost
    if candidate.is_interactive {
        score += 0.05;
    }

    score.clamp(0.0, 1.0)
}

fn fuzzy_ratio(a: &str, b: &str) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    TextDiff::from_chars(a, b).ratio() as f32
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn collect_backend_node_ids(node: &Value, ids: &mut HashSet<i64>) {
    if let Some(bid) = node.get("backendNodeId").and_then(|v| v.as_i64()) {
        ids.insert(bid);
    }
    if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
        for child in children {
            collect_backend_node_ids(child, ids);
        }
    }
    // Also check shadow roots and content documents
    if let Some(shadow) = node.get("shadowRoots").and_then(|v| v.as_array()) {
        for child in shadow {
            collect_backend_node_ids(child, ids);
        }
    }
    if let Some(content_doc) = node.get("contentDocument") {
        collect_backend_node_ids(content_doc, ids);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(role: &str, name: &str) -> SemanticCandidate {
        SemanticCandidate {
            role: role.to_string(),
            name: name.to_string(),
            value_text: None,
            backend_node_id: None,
            is_interactive: INTERACTIVE_ROLES.contains(&role),
        }
    }

    fn candidate_with_value(role: &str, name: &str, value: &str) -> SemanticCandidate {
        SemanticCandidate {
            role: role.to_string(),
            name: name.to_string(),
            value_text: Some(value.to_string()),
            backend_node_id: None,
            is_interactive: INTERACTIVE_ROLES.contains(&role),
        }
    }

    #[test]
    fn exact_match_scores_highest() {
        let c = candidate("button", "Start a post");
        let score = score_candidate(&c, "Start a post", "start a post", None);
        assert!(
            score > 0.95,
            "Exact match should score > 0.95, got {}",
            score
        );
    }

    #[test]
    fn case_insensitive_exact_match() {
        let c = candidate("button", "Start a Post");
        let score = score_candidate(&c, "start a post", "start a post", None);
        assert!(
            score > 0.95,
            "Case-insensitive exact match should score > 0.95, got {}",
            score
        );
    }

    #[test]
    fn substring_match_name_contains_query() {
        let c = candidate("button", "Click here to Start a post about something");
        let score = score_candidate(&c, "Start a post", "start a post", None);
        assert!(
            score > 0.4 && score < 0.8,
            "Substring match should be 0.4-0.8, got {}",
            score
        );
    }

    #[test]
    fn query_contains_name() {
        let c = candidate("button", "Start a post");
        let score = score_candidate(
            &c,
            "Start a post button on the page",
            "start a post button on the page",
            None,
        );
        assert!(
            score > 0.35,
            "Query-contains-name should score > 0.35, got {}",
            score
        );
    }

    #[test]
    fn fuzzy_match_with_typo() {
        let c = candidate("button", "Start a post");
        let score = score_candidate(&c, "Start a pst", "start a pst", None);
        assert!(
            score > 0.3,
            "Fuzzy match with typo should score > 0.3, got {}",
            score
        );
    }

    #[test]
    fn role_hint_boosts_matching_role() {
        let btn = candidate("button", "Submit");
        let link = candidate("link", "Submit");
        let btn_score = score_candidate(&btn, "Submit", "submit", Some("button"));
        let link_score = score_candidate(&link, "Submit", "submit", Some("button"));
        assert!(
            btn_score > link_score,
            "Role hint should boost button: btn={} link={}",
            btn_score,
            link_score
        );
    }

    #[test]
    fn empty_name_scores_zero() {
        let c = candidate("generic", "");
        let score = score_candidate(&c, "Start a post", "start a post", None);
        assert!(
            score < 0.01,
            "Empty name should score near zero, got {}",
            score
        );
    }

    #[test]
    fn value_text_match() {
        let c = candidate_with_value("textbox", "Search", "Find something");
        let score = score_candidate(&c, "Find something", "find something", None);
        assert!(
            score >= 0.8,
            "Value text exact match should score >= 0.8, got {}",
            score
        );
    }

    #[test]
    fn prefix_match() {
        let c = candidate("button", "Start a post");
        let score = score_candidate(&c, "Start", "start", None);
        assert!(
            score > 0.6,
            "Prefix match should score > 0.6, got {}",
            score
        );
    }

    #[test]
    fn interactive_boost() {
        let interactive = SemanticCandidate {
            role: "button".to_string(),
            name: "Submit".to_string(),
            value_text: None,
            backend_node_id: None,
            is_interactive: true,
        };
        let non_interactive = SemanticCandidate {
            role: "heading".to_string(),
            name: "Submit".to_string(),
            value_text: None,
            backend_node_id: None,
            is_interactive: false,
        };
        let s1 = score_candidate(&interactive, "Submit", "submit", None);
        let s2 = score_candidate(&non_interactive, "Submit", "submit", None);
        assert!(
            s1 > s2,
            "Interactive should score higher: {} vs {}",
            s1,
            s2
        );
    }

    #[test]
    fn fuzzy_ratio_identical() {
        assert!((fuzzy_ratio("hello", "hello") - 1.0).abs() < 0.01);
    }

    #[test]
    fn fuzzy_ratio_empty() {
        assert!(fuzzy_ratio("hello", "").abs() < 0.01);
        assert!((fuzzy_ratio("", "") - 1.0).abs() < 0.01);
    }

    #[test]
    fn fuzzy_ratio_similar() {
        let r = fuzzy_ratio("start a post", "start a pst");
        assert!(r > 0.8, "Similar strings should have high ratio: {}", r);
    }

    #[test]
    fn fuzzy_ratio_different() {
        let r = fuzzy_ratio("start a post", "completely different text");
        assert!(r < 0.5, "Different strings should have low ratio: {}", r);
    }
}
