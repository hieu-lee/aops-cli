use std::sync::OnceLock;

/// (code as used by imo-official.org (differs from ISO 3166 for some, e.g. GER, HEL, SUI), AoPS country-folder name, extra aliases/demonyms)
pub const COUNTRIES: &[(&str, &str, &[&str])] = &[
    (
        "USA",
        "USA",
        &[
            "united states",
            "united states of america",
            "us",
            "america",
            "usamo",
            "usajmo",
            "tstst",
        ],
    ),
    ("ALB", "Albania", &["albanian"]),
    ("ALG", "Algeria", &[]),
    ("ARG", "Argentina", &["argentine", "argentinian"]),
    ("ARM", "Armenia", &["armenian"]),
    ("AUS", "Australia", &["australian"]),
    ("AUT", "Austria", &["austrian"]),
    ("AZE", "Azerbaijan", &["azerbaijani"]),
    ("BGD", "Bangladesh", &["bangladeshi"]),
    ("BLR", "Belarus", &["belarusian"]),
    ("BEL", "Belgium", &["flanders"]),
    ("BOL", "Bolivia", &[]),
    ("BIH", "Bosnia Herzegovina", &["bosnia", "bosnia and herzegovina"]),
    ("BRA", "Brazil", &["brazilian"]),
    ("BGR", "Bulgaria", &["bulgarian"]),
    ("CAN", "Canada", &["canadian"]),
    ("CHI", "Chile", &[]),
    ("CHN", "China", &["prc", "people's republic of china", "chinese"]),
    ("COL", "Colombia", &["colombian"]),
    ("CRI", "Costa Rica", &[]),
    ("HRV", "Croatia", &["croatian"]),
    ("CUB", "Cuba", &["cuban"]),
    ("CYP", "Cyprus", &[]),
    ("CZE", "Czech Republic", &["czechia", "czech"]),
    ("DEN", "Denmark", &["danish"]),
    ("ECU", "Ecuador", &[]),
    ("SLV", "El Salvador", &[]),
    ("EST", "Estonia", &["estonian"]),
    ("FIN", "Finland", &["finnish"]),
    ("FRA", "France", &["french"]),
    ("GEO", "Georgia", &["georgian"]),
    ("GER", "Germany", &["german"]),
    ("HEL", "Greece", &["greek"]),
    ("HND", "Honduras", &[]),
    ("HKG", "Hong Kong", &[]),
    ("HUN", "Hungary", &["kurschak", "kürschák", "hungarian"]),
    ("IND", "India", &["indian"]),
    ("IDN", "Indonesia", &["indonesian"]),
    ("IRN", "Iran", &["iranian"]),
    ("IRL", "Ireland", &["irish"]),
    ("ISR", "Israel", &["israeli"]),
    ("ITA", "Italy", &["italian", "itamo"]),
    ("JPN", "Japan", &["japanese"]),
    ("KAZ", "Kazakhstan", &["kazakh"]),
    ("KOR", "Korea", &["south korea", "republic of korea", "korean"]),
    ("KSV", "Kosovo", &[]),
    ("KGZ", "Kyrgyzstan", &[]),
    ("LVA", "Latvia", &["latvian"]),
    ("LTU", "Lithuania", &["lithuanian"]),
    ("MAS", "Malaysia", &["malaysian"]),
    ("MEX", "Mexico", &["mexican"]),
    ("MDA", "Moldova", &["moldovan"]),
    ("MNG", "Mongolia", &["mongolian"]),
    ("MAR", "Morocco", &["moroccan"]),
    ("MOZ", "Mozambique", &[]),
    ("MMR", "Myanmar", &[]),
    ("NPL", "Nepal", &["nepali"]),
    ("NLD", "Netherlands", &["dutch"]),
    ("NZL", "New Zealand", &[]),
    ("NGA", "Nigeria", &[]),
    ("PRK", "North Korea", &["dprk"]),
    ("MKD", "North Macedonia", &["macedonia"]),
    ("NOR", "Norway", &["norwegian"]),
    ("PAK", "Pakistan", &["pakistani"]),
    ("PAR", "Paraguay", &[]),
    ("PER", "Peru", &["peruvian"]),
    ("PHI", "Philippines", &["the philippines", "philippine"]),
    ("POL", "Poland", &["polish"]),
    ("POR", "Portugal", &["portuguese"]),
    ("PRI", "Puerto Rico", &[]),
    ("ROU", "Romania", &["romanian"]),
    ("RUS", "Russia", &["russian federation", "all-russian", "russian"]),
    ("SAU", "Saudi Arabia", &["saudi"]),
    ("SRB", "Serbia", &["serbian"]),
    ("SGP", "Singapore", &["singaporean"]),
    ("SVN", "Slovenia", &["slovenian"]),
    ("SAF", "South Africa", &[]),
    ("ESP", "Spain", &["spanish"]),
    ("LKA", "Sri Lanka", &[]),
    ("SWE", "Sweden", &["swedish"]),
    ("SUI", "Switzerland", &["swiss"]),
    ("SYR", "Syria", &[]),
    ("TWN", "Taiwan", &["chinese taipei"]),
    ("TJK", "Tajikistan", &[]),
    ("THA", "Thailand", &["thai"]),
    ("TUR", "Turkey", &["turkiye", "türkiye", "turkish"]),
    ("TKM", "Turkmenistan", &[]),
    ("UKR", "Ukraine", &["ukrainian"]),
    ("URY", "Uruguay", &[]),
    ("UZB", "Uzbekistan", &[]),
    ("VNM", "Vietnam", &["viet nam", "vietnamese"]),
    ("SVK", "Slovakia", &["slovak"]),
    ("TUN", "Tunisia", &[]),
    ("EGY", "Egypt", &[]),
    ("LUX", "Luxembourg", &[]),
    ("ISL", "Iceland", &[]),
    ("MAC", "Macau", &["macao"]),
    ("LBN", "Lebanon", &[]),
    ("UNK", "United Kingdom", &["gbr", "uk", "britain", "great britain", "british"]),
];

fn terms() -> &'static Vec<(String, &'static str)> {
    static T: OnceLock<Vec<(String, &'static str)>> = OnceLock::new();
    T.get_or_init(|| {
        let mut v: Vec<(String, &'static str)> = vec![];
        for (iso, key, al) in COUNTRIES {
            v.push((key.to_lowercase(), *iso));
            v.extend(al.iter().map(|a| (a.to_lowercase(), *iso)));
        }
        v.sort_by_key(|(t, _)| std::cmp::Reverse(t.len()));
        v
    })
}

fn has_word(hay: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(i) = hay[from..].find(needle) {
        let s = from + i;
        let e = s + needle.len();
        let before = hay[..s].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = hay[e..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before && after {
            return true;
        }
        from = s + needle.len().max(1);
    }
    false
}

/// Country mentioned in a contest name ("China Team Selection Test" -> CHN).
pub fn infer(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    terms().iter().find(|(t, _)| has_word(&n, t)).map(|(_, iso)| *iso)
}

/// Resolve user input (ISO3, name, alias, or unique name fragment) to ISO3.
/// Standard ISO 3166 codes that differ from the IMO ones.
const ISO_STD: &[(&str, &str)] = &[
    ("DEU", "GER"),
    ("GRC", "HEL"),
    ("PHL", "PHI"),
    ("MYS", "MAS"),
    ("CHE", "SUI"),
    ("PRT", "POR"),
    ("ZAF", "SAF"),
    ("DNK", "DEN"),
    ("DZA", "ALG"),
    ("KOS", "KSV"),
    ("PRY", "PAR"),
    ("CHL", "CHI"),
    ("GBR", "UNK"),
];

pub fn resolve(input: &str) -> Result<&'static str, String> {
    let q = input.trim().to_lowercase();
    if let Some((_, imo)) = ISO_STD.iter().find(|(std, _)| std.to_lowercase() == q) {
        return Ok(COUNTRIES.iter().find(|c| c.0 == *imo).map_or(imo, |c| c.0));
    }
    if let Some((iso, _)) = terms().iter().map(|(t, i)| (i, t)).find(|(i, t)| **t == q || i.to_lowercase() == q) {
        return Ok(iso);
    }
    let mut hits: Vec<&'static str> = COUNTRIES
        .iter()
        .filter(|(_, k, _)| k.to_lowercase().contains(&q))
        .map(|c| c.0)
        .collect();
    hits.dedup();
    match hits.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("unknown country '{input}' (use ISO3 like CHN or a name like china)")),
        many => Err(format!("ambiguous country '{input}': {}", many.join(","))),
    }
}

pub fn key(iso: &str) -> Option<&'static str> {
    COUNTRIES.iter().find(|c| c.0 == iso).map(|c| c.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn infers() {
        assert_eq!(infer("China Team Selection Test"), Some("CHN"));
        assert_eq!(infer("North Korea TST"), Some("PRK"));
        assert_eq!(infer("Korea - Final Round"), Some("KOR"));
        assert_eq!(infer("All-Russian Olympiad"), Some("RUS"));
        assert_eq!(infer("USAMO"), Some("USA"));
        assert_eq!(infer("Dutch IMO TST"), Some("NLD"));
        assert_eq!(infer("IMO Shortlist"), None);
    }
    #[test]
    fn resolves() {
        assert_eq!(resolve("chn"), Ok("CHN"));
        assert_eq!(resolve("Iran"), Ok("IRN"));
        assert_eq!(resolve("united states"), Ok("USA"));
        assert_eq!(resolve("Germany"), Ok("GER"));
        assert_eq!(resolve("deu"), Ok("GER"));
        assert_eq!(resolve("SUI"), Ok("SUI"));
        assert_eq!(key("HEL"), Some("Greece"));
        assert_eq!(resolve("hong"), Ok("HKG"));
        assert!(resolve("zzz").is_err());
    }
}
