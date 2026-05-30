//! Shared SEC helpers: ticker → CIK lookup, User-Agent constants, rate-limit
//! pacing (SEC's published cap is 10 req/s with a descriptive UA).

/// Hardcoded ticker → 10-digit CIK map for the seed Tier-1 set. When the
/// system grows past ~25 names this should move to a DB column on `ticker`
/// (or fetch from <https://www.sec.gov/files/company_tickers.json>); for now
/// the small static table is enough and keeps the adapters dependency-free.
const SEED: &[(&str, &str)] = &[
    ("NVDA", "0001045810"),
    ("MU",   "0000723125"),
    ("AMD",  "0000002488"),
    ("AMAT", "0000006951"),
    ("TSM",  "0001046179"),
    ("ANET", "0001596532"),
    ("VRT",  "0001674101"),
    ("CDNS", "0000813672"),
];

#[must_use]
pub fn cik_for(symbol: &str) -> Option<&'static str> {
    let up = symbol.to_ascii_uppercase();
    SEED.iter().find(|(s, _)| *s == up.as_str()).map(|(_, c)| *c)
}

#[must_use]
pub fn all_seeded() -> impl Iterator<Item = (&'static str, &'static str)> {
    SEED.iter().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tickers_resolve() {
        assert_eq!(cik_for("NVDA"), Some("0001045810"));
        assert_eq!(cik_for("nvda"), Some("0001045810"), "case insensitive");
        assert_eq!(cik_for("UNKNOWN"), None);
    }
}
