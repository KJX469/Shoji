//! Breach checks for saved passwords through Have I Been Pwned's range API: only the first five
//! characters of a password's SHA-1 hash leave the PC, and the answer lists every known hash
//! that starts with them.

use std::collections::{BTreeMap, HashMap};

/// The range API; the five-character prefix is appended.
pub const RANGE_URL: &str = "https://api.pwnedpasswords.com/range/";

/// The SHA-1 of `password`, as 40 uppercase hex characters (the form the API uses).
pub fn sha1_hex(password: &str) -> String {
    let d = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, password.as_bytes());
    d.as_ref().iter().map(|b| format!("{b:02X}")).collect()
}

/// `hashes` grouped by the prefix sent for them, so each prefix is asked for once.
pub fn by_prefix<'a>(hashes: impl IntoIterator<Item = &'a str>) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for h in hashes {
        if h.len() != 40 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let (prefix, suffix) = h.split_at(5);
        let list = out.entry(prefix.to_ascii_uppercase()).or_default();
        let suffix = suffix.to_ascii_uppercase();
        if !list.contains(&suffix) {
            list.push(suffix);
        }
    }
    out
}

/// How often each of `suffixes` appears in breaches, from the API's answer for their prefix
/// (`SUFFIX:COUNT` lines). Padding lines have a count of 0 and are skipped; a suffix that isn't
/// listed has never been seen.
pub fn counts(body: &str, suffixes: &[String]) -> HashMap<String, u64> {
    let mut out: HashMap<String, u64> = suffixes.iter().map(|s| (s.clone(), 0)).collect();
    for line in body.lines() {
        let Some((suffix, count)) = line.trim().split_once(':') else {
            continue;
        };
        let suffix = suffix.to_ascii_uppercase();
        if let (Some(slot), Ok(n)) = (out.get_mut(&suffix), count.trim().parse::<u64>()) {
            *slot = n;
        }
    }
    out
}

/// Check `hashes` (full SHA-1 hex), asking `fetch` for each prefix's answer. Returns the count
/// for every hash whose prefix was answered; hashes whose fetch failed are left out.
pub fn check<'a>(
    hashes: impl IntoIterator<Item = &'a str>,
    mut fetch: impl FnMut(&str) -> Option<String>,
) -> HashMap<String, u64> {
    let mut out = HashMap::new();
    for (prefix, suffixes) in by_prefix(hashes) {
        let Some(body) = fetch(&prefix) else {
            continue;
        };
        for (suffix, n) in counts(&body, &suffixes) {
            out.insert(format!("{prefix}{suffix}"), n);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_match_the_api_form() {
        assert_eq!(
            sha1_hex("password"),
            "5BAA61E4C9B93F3F0682250B6CF8331B7EE68FD8"
        );
    }

    #[test]
    fn only_the_prefix_is_sent_and_each_once() {
        let a = sha1_hex("password");
        let b = sha1_hex("correct horse battery staple");
        let mut asked = Vec::new();
        let found = check([a.as_str(), a.as_str(), b.as_str()], |prefix| {
            asked.push(prefix.to_string());
            Some(match prefix {
                "5BAA6" => "003D68EB55068C33ACE09247EE4C639306B:3\r\n\
                            1E4C9B93F3F0682250B6CF8331B7EE68FD8:10434004\r\n\
                            FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF:0"
                    .to_string(),
                _ => return None,
            })
        });
        assert_eq!(asked.len(), 2);
        assert!(asked.iter().all(|p| p.len() == 5));
        assert_eq!(found.get(&a), Some(&10434004));
        // A failed lookup is left out rather than reported as safe.
        assert_eq!(found.get(&b), None);
    }

    #[test]
    fn unlisted_and_padding_suffixes_count_zero() {
        let s = vec!["AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string()];
        assert_eq!(
            counts("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:0\nBBB:4", &s)[&s[0]],
            0
        );
        assert_eq!(counts("", &s)[&s[0]], 0);
        assert!(by_prefix(["not a hash"]).is_empty());
    }
}
