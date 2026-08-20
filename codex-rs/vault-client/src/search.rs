//! Searching an open vault, entirely on this machine.
//!
//! The server holds encrypted vectors and encrypted text, so it can rank nothing and
//! filter nothing. Every comparison here is on data this process decrypted -- which is the
//! practical face of blind storage rather than a limitation of it.
//!
//! The scoring must agree with the dashboard's `vector.js`: vectors are quantized with a
//! per-vector scale and compared with a true cosine, so the scale cancels in the division.
//! Scoring them any other way would rank the same query differently in the two clients.
use crate::vault::MemoryEntry;

/// Euclidean norm of a quantized vector. Derived, never stored.
pub fn vector_norm(vector: &[i8]) -> f64 {
    vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt()
}

/// Cosine similarity, -1..1. Zero-length input scores 0 rather than dividing by zero.
pub fn similarity(left: &[i8], right: &[i8]) -> f64 {
    if left.len() != right.len() {
        return -1.0;
    }
    let dot: f64 = left
        .iter()
        .zip(right)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum();
    let denominator = vector_norm(left) * vector_norm(right);
    if denominator == 0.0 { 0.0 } else { dot / denominator }
}

/// Quantize a float embedding to int8 with a per-vector scale.
///
/// Matching `quantizeInt8` in the dashboard: unit-normalising and multiplying by 127 wastes
/// most of the int8 range at high dimensions, so each vector is scaled by its own largest
/// component instead. The search above divides by each vector's norm, so that scale cancels.
pub fn quantize(vector: &[f32]) -> Vec<i8> {
    let peak = vector.iter().copied().map(f32::abs).fold(0.0f32, f32::max);
    let scale = if peak == 0.0 { 1.0 } else { 127.0 / peak };
    vector
        .iter()
        .map(|value| (value * scale).round().clamp(-127.0, 127.0) as i8)
        .collect()
}

#[derive(Debug, Clone)]
pub struct SearchHit<'a> {
    pub entry: &'a MemoryEntry,
    pub score: f64,
}

/// Nearest neighbours over the decrypted index.
///
/// `min_score` defaults to 0 at the callsite: a memory pointing the opposite way in
/// embedding space is not a result worth returning.
pub fn by_vector<'a>(
    entries: &'a [MemoryEntry],
    query: &[i8],
    limit: usize,
    min_score: f64,
) -> Vec<SearchHit<'a>> {
    let mut hits: Vec<SearchHit<'a>> = entries
        .iter()
        .filter_map(|entry| {
            let vector = entry.vector.as_ref()?;
            let score = similarity(query, vector);
            (score >= min_score).then_some(SearchHit { entry, score })
        })
        .collect();
    hits.sort_by(|left, right| right.score.total_cmp(&left.score));
    hits.truncate(limit);
    hits
}

/// Substring search, for a vault with no vector index and for exact phrases someone
/// remembers typing.
pub fn by_text<'a>(entries: &'a [MemoryEntry], query: &str, limit: usize) -> Vec<SearchHit<'a>> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<SearchHit<'a>> = entries
        .iter()
        .filter_map(|entry| {
            let title = entry.payload.title.to_lowercase();
            let haystack = format!(
                "{title}\n{}\n{}",
                entry.payload.body.to_lowercase(),
                entry.payload.tags.join(" ").to_lowercase()
            );
            let at = haystack.find(&needle)?;
            // A title hit outranks a body hit, and an earlier match outranks a later one.
            let base = if title.contains(&needle) { 1.0 } else { 0.5 };
            let score = base - (at as f64) / ((haystack.len() as f64).max(1.0) * 100.0);
            Some(SearchHit { entry, score })
        })
        .collect();
    hits.sort_by(|left, right| right.score.total_cmp(&left.score));
    hits.truncate(limit);
    hits
}

/// Merge vector and text hits.
///
/// Both, not one or the other: an exact phrase the user remembers typing should not be
/// pushed out of the results by a vector that merely means something similar.
pub fn combined<'a>(
    entries: &'a [MemoryEntry],
    query: &str,
    query_vector: Option<&[i8]>,
    limit: usize,
) -> Vec<SearchHit<'a>> {
    let mut best: Vec<SearchHit<'a>> = Vec::new();
    let mut push = |hit: SearchHit<'a>| {
        match best.iter_mut().find(|existing| existing.entry.id == hit.entry.id) {
            Some(existing) => {
                if hit.score > existing.score {
                    existing.score = hit.score;
                }
            }
            None => best.push(hit),
        }
    };

    if let Some(query_vector) = query_vector {
        for hit in by_vector(entries, query_vector, limit * 2, 0.0) {
            push(hit);
        }
    }
    for hit in by_text(entries, query, limit * 2) {
        push(hit);
    }

    best.sort_by(|left, right| right.score.total_cmp(&left.score));
    best.truncate(limit);
    best
}
