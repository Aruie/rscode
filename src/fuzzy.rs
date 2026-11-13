//! Subsequence fuzzy matching for Quick Open and the Command Palette.

/// Higher is better; None if `query` is not a subsequence of `cand`.
pub fn score(query: &str, cand: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect();
    let c: Vec<char> = cand.chars().collect();
    let cl: Vec<char> = c.iter().map(|ch| ch.to_lowercase().next().unwrap_or(*ch)).collect();
    let mut qi = 0;
    let mut s = 0;
    let mut prev: Option<usize> = None;
    for (i, ch) in cl.iter().enumerate() {
        if qi < q.len() && *ch == q[qi] {
            s += 1;
            if prev == Some(i.wrapping_sub(1)) {
                s += 5;
            }
            let boundary = i == 0 || matches!(c[i - 1], '/' | '\\' | '_' | '-' | '.' | ' ' | ':') || (c[i].is_uppercase() && c[i - 1].is_lowercase());
            if boundary {
                s += 8;
            }
            prev = Some(i);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    // acronym bonus: query matches word starts ("tlc" -> Toggle Line Comment)
    let starts: Vec<char> = (0..c.len())
        .filter(|&i| i == 0 || matches!(c[i - 1], '/' | '\\' | '_' | '-' | '.' | ' ' | ':') || (c[i].is_uppercase() && c[i - 1].is_lowercase()))
        .map(|i| cl[i])
        .collect();
    let mut k = 0;
    for ch in &starts {
        if k < q.len() && *ch == q[k] {
            k += 1;
        }
    }
    if k == q.len() {
        s += 10 * q.len() as i32;
    }
    Some(s * 10 - c.len() as i32)
}

/// Quick Open: prefer matches in the file name over the directory part.
pub fn score_path(query: &str, rel: &str) -> Option<i32> {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    match score(query, name) {
        Some(s) => Some(s + 1000),
        None => score(query, rel),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_filename_and_boundaries() {
        assert!(score("mr", "main.rs").is_some());
        assert!(score("xyz", "main.rs").is_none());
        assert!(score_path("main", "src/main.rs") > score_path("main", "domain/x.rs"));
        assert!(score("tlc", "Toggle Line Comment") > score("tlc", "tool collection"));
    }
}
