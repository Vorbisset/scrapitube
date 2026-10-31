//! Lecture et affichage des temps (« 1:30 », « 01:02:03 », « 90 »…).

/// Lit un temps saisi : secondes (« 90 », « 90.5 »), « m:ss » ou « h:mm:ss ».
pub fn parse(text: &str) -> Option<f64> {
    let text = text.trim().replace(',', ".");
    if text.is_empty() {
        return None;
    }
    let mut total = 0.0;
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    for (i, part) in parts.iter().enumerate() {
        let value: f64 = part.parse().ok().filter(|v: &f64| *v >= 0.0)?;
        // Les minutes et secondes après un « : » ne dépassent pas 59.
        if i > 0 && value >= 60.0 {
            return None;
        }
        total = total * 60.0 + value;
    }
    Some(total)
}

/// « 1:05 » ou « 1:02:03 ».
pub fn format(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Version sans « : » (interdit dans les noms de fichiers sur Mac) : « 1m05 » ou « 1h02m03 ».
pub fn for_filename(seconds: f64) -> String {
    format(seconds).replacen(':', if seconds >= 3600.0 { "h" } else { "m" }, 1).replace(':', "m")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        assert_eq!(parse("90"), Some(90.0));
        assert_eq!(parse("1:30"), Some(90.0));
        assert_eq!(parse("01:02:03"), Some(3723.0));
        assert_eq!(parse("1:30,5"), Some(90.5));
        assert_eq!(parse("1:75"), None);
        assert_eq!(parse("abc"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn formats_times() {
        assert_eq!(format(65.0), "1:05");
        assert_eq!(format(3723.0), "1:02:03");
        assert_eq!(for_filename(65.0), "1m05");
        assert_eq!(for_filename(3723.0), "1h02m03");
    }
}
