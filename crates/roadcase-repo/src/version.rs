//! Comparing package versions as dpkg and pacman do.

use std::cmp::Ordering;

/// dpkg's order of two versions (`[epoch:]upstream[-revision]`).
pub fn compare_debian(a: &str, b: &str) -> Ordering {
    fn split(version: &str) -> (u64, &str, &str) {
        let (epoch, rest) = match version.split_once(':') {
            Some((epoch, rest)) if epoch.bytes().all(|c| c.is_ascii_digit()) => {
                (epoch.parse().unwrap_or(0), rest)
            }
            _ => (0, version),
        };
        match rest.rsplit_once('-') {
            Some((upstream, revision)) => (epoch, upstream, revision),
            None => (epoch, rest, ""),
        }
    }
    let (epoch_a, upstream_a, revision_a) = split(a);
    let (epoch_b, upstream_b, revision_b) = split(b);
    epoch_a
        .cmp(&epoch_b)
        .then_with(|| dpkg_part(upstream_a, upstream_b))
        .then_with(|| dpkg_part(revision_a, revision_b))
}

/// dpkg's `verrevcmp`.
fn dpkg_part(a: &str, b: &str) -> Ordering {
    fn order(c: Option<u8>) -> i32 {
        match c {
            None => 0,
            Some(b'~') => -1,
            Some(c) if c.is_ascii_digit() => 0,
            Some(c) if c.is_ascii_alphabetic() => i32::from(c),
            Some(c) => i32::from(c) + 256,
        }
    }
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let mut first_diff = 0;
        while (i < a.len() && !a[i].is_ascii_digit()) || (j < b.len() && !b[j].is_ascii_digit()) {
            let ac = order(a.get(i).copied());
            let bc = order(b.get(j).copied());
            if ac != bc {
                return ac.cmp(&bc);
            }
            i += 1;
            j += 1;
        }
        while i < a.len() && a[i] == b'0' {
            i += 1;
        }
        while j < b.len() && b[j] == b'0' {
            j += 1;
        }
        while i < a.len() && a[i].is_ascii_digit() && j < b.len() && b[j].is_ascii_digit() {
            if first_diff == 0 {
                first_diff = i32::from(a[i]) - i32::from(b[j]);
            }
            i += 1;
            j += 1;
        }
        if i < a.len() && a[i].is_ascii_digit() {
            return Ordering::Greater;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            return Ordering::Less;
        }
        if first_diff != 0 {
            return first_diff.cmp(&0);
        }
    }
    Ordering::Equal
}

/// pacman's order of two versions (`[epoch:]pkgver[-pkgrel]`), as
/// `vercmp` and `alpm_pkg_vercmp` give it.
pub fn compare_pacman(a: &str, b: &str) -> Ordering {
    fn split(version: &str) -> (&str, &str, Option<&str>) {
        let (epoch, rest) = match version.split_once(':') {
            Some((epoch, rest)) if epoch.bytes().all(|c| c.is_ascii_digit()) => (epoch, rest),
            _ => ("0", version),
        };
        match rest.rsplit_once('-') {
            Some((version, release)) => (epoch, version, Some(release)),
            None => (epoch, rest, None),
        }
    }
    if a == b {
        return Ordering::Equal;
    }
    let (epoch_a, version_a, release_a) = split(a);
    let (epoch_b, version_b, release_b) = split(b);
    rpm_compare(epoch_a, epoch_b)
        .then_with(|| rpm_compare(version_a, version_b))
        .then_with(|| match (release_a, release_b) {
            (Some(a), Some(b)) => rpm_compare(a, b),
            _ => Ordering::Equal,
        })
}

/// libalpm's `rpmvercmp`.
fn rpm_compare(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut one, mut two) = (0, 0);
    while one < a.len() && two < b.len() {
        let (start_one, start_two) = (one, two);
        while one < a.len() && !a[one].is_ascii_alphanumeric() {
            one += 1;
        }
        while two < b.len() && !b[two].is_ascii_alphanumeric() {
            two += 1;
        }
        if one >= a.len() || two >= b.len() {
            break;
        }
        // Separators of different lengths: the longer is newer.
        if one - start_one != two - start_two {
            return (one - start_one).cmp(&(two - start_two));
        }
        let (segment_one, segment_two) = (one, two);
        let numeric = a[one].is_ascii_digit();
        let take = |bytes: &[u8], mut at: usize| {
            while at < bytes.len()
                && (if numeric {
                    bytes[at].is_ascii_digit()
                } else {
                    bytes[at].is_ascii_alphabetic()
                })
            {
                at += 1;
            }
            at
        };
        one = take(a, one);
        two = take(b, two);
        if segment_two == two {
            // A number is newer than letters.
            return if numeric {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }
        let (mut x, mut y) = (&a[segment_one..one], &b[segment_two..two]);
        if numeric {
            while x.first() == Some(&b'0') {
                x = &x[1..];
            }
            while y.first() == Some(&b'0') {
                y = &y[1..];
            }
            if x.len() != y.len() {
                return x.len().cmp(&y.len());
            }
        }
        match x.cmp(y) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    let (rest_one, rest_two) = (&a[one.min(a.len())..], &b[two.min(b.len())..]);
    if rest_one.is_empty() && rest_two.is_empty() {
        return Ordering::Equal;
    }
    // A remaining letter is older than nothing, and nothing older than a
    // remaining number.
    let alpha = |rest: &[u8]| rest.first().is_some_and(u8::is_ascii_alphabetic);
    if (rest_one.is_empty() && !alpha(rest_two)) || alpha(rest_one) {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Ordering::{Equal, Greater, Less};

    #[test]
    fn dpkg_orders_versions() {
        for (a, b, order) in [
            ("1.0-1", "1.0-1", Equal),
            ("1.0-1", "1.0-2", Less),
            ("1.10-1", "1.9-1", Greater),
            ("1.2.0~beta.1-1", "1.2.0-1", Less),
            ("1.2.0~beta.1-1", "1.2.0~beta.2-1", Less),
            ("1.2.0-1", "1.2.0+b1-1", Less),
            ("1:0.1-1", "9.9-1", Greater),
            ("1.0a-1", "1.0-1", Greater),
            ("2.0.0-1", "10.0.0-1", Less),
        ] {
            assert_eq!(compare_debian(a, b), order, "{a} vs {b}");
            assert_eq!(compare_debian(b, a), order.reverse(), "{b} vs {a}");
        }
    }

    #[test]
    fn pacman_orders_versions() {
        for (a, b, order) in [
            ("1.0-1", "1.0-1", Equal),
            ("1.0-1", "1.0-2", Less),
            ("1.10-1", "1.9-1", Greater),
            ("1.2.0beta.1-1", "1.2.0-1", Less),
            ("1.2.0beta.1-1", "1.2.0beta.2-1", Less),
            ("1.2.0alpha-1", "1.2.0beta-1", Less),
            ("1:0.1-1", "9.9-1", Greater),
            ("1.0", "1.0-5", Equal),
            ("1.0.1", "1.0", Greater),
            ("1.0a", "1.0", Less),
            ("2.0.0-1", "10.0.0-1", Less),
        ] {
            assert_eq!(compare_pacman(a, b), order, "{a} vs {b}");
            assert_eq!(compare_pacman(b, a), order.reverse(), "{b} vs {a}");
        }
    }
}
