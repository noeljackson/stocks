use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateRank {
    pub score: f64,
    pub bucket: &'static str,
    pub reasons: Vec<String>,
}

#[must_use]
pub fn rank_candidate(
    signal_name: &str,
    signal_value: Option<f64>,
    domain_fit: Option<f64>,
    parent_theme_fit: Option<f64>,
    proposed_tier: i32,
    proposed_lists: &Value,
    has_suggested_new_list: bool,
) -> CandidateRank {
    let mut score = signal_base(signal_name);
    let mut reasons = vec![signal_reason(signal_name).to_string()];

    let strength = signal_strength(signal_name, signal_value);
    if signal_name == "research_nomination" {
        if strength >= 12.0 {
            reasons.push("evidence ready".to_string());
        } else if strength >= 8.0 {
            reasons.push("evidence mostly ready".to_string());
        }
    } else if strength >= 12.0 {
        reasons.push("strong signal value".to_string());
    }
    score += strength;

    if let Some(domain_fit) = domain_fit {
        let domain_points = (domain_fit.clamp(0.0, 100.0) / 100.0) * 15.0;
        score += domain_points;
        if domain_fit >= 80.0 {
            reasons.push(format!("strong theme fit {}", domain_fit.round() as i64));
        } else if signal_name == "research_nomination" && domain_fit >= 60.0 {
            reasons.push(format!("theme fit {}", domain_fit.round() as i64));
        }
    }

    if let Some(parent_theme_fit) = parent_theme_fit {
        let theme_points = (parent_theme_fit.clamp(0.0, 100.0) / 100.0) * 12.0;
        score += theme_points;
        if parent_theme_fit >= 70.0 {
            reasons.push(format!(
                "active parent theme fit {}",
                parent_theme_fit.round() as i64
            ));
        } else if parent_theme_fit >= 50.0 {
            reasons.push(format!(
                "parent theme fit {}",
                parent_theme_fit.round() as i64
            ));
        }
    }

    match proposed_tier {
        1 => {
            score += 10.0;
            reasons.push("tier 1 candidate".to_string());
        }
        2 => score += 6.0,
        _ => score += 2.0,
    }

    let list_fit = list_fit_score(proposed_lists);
    score += list_fit;
    if list_fit >= 8.0 {
        reasons.push("high-confidence watchlist fit".to_string());
    }

    if has_suggested_new_list {
        score += 3.0;
        reasons.push("suggests new watchlist".to_string());
    }

    let score = score.clamp(0.0, 100.0);
    let bucket = if score >= 75.0 {
        "highest"
    } else if score >= 60.0 {
        "high"
    } else if score >= 40.0 {
        "medium"
    } else {
        "low"
    };

    CandidateRank {
        score,
        bucket,
        reasons,
    }
}

fn signal_base(signal_name: &str) -> f64 {
    match signal_name {
        "estimate_revision_velocity" => 45.0,
        "base_breakout" => 42.0,
        "news_sentiment_shift" => 38.0,
        "research_nomination" => 34.0,
        "volume_anomaly" => 20.0,
        _ => 18.0,
    }
}

fn signal_reason(signal_name: &str) -> &'static str {
    match signal_name {
        "estimate_revision_velocity" => "estimate revisions",
        "base_breakout" => "base breakout",
        "news_sentiment_shift" => "news sentiment shift",
        "research_nomination" => "research nomination",
        "volume_anomaly" => "volume anomaly",
        _ => "discovery signal",
    }
}

fn signal_strength(signal_name: &str, signal_value: Option<f64>) -> f64 {
    let Some(value) = signal_value else {
        return if signal_name == "research_nomination" {
            8.0
        } else {
            0.0
        };
    };
    match signal_name {
        // Research nominations use signal_value as available-source count
        // (price/news/estimates/fundamentals), not a market z-score.
        "research_nomination" => ((value.clamp(0.0, 4.0) / 4.0) * 14.0).clamp(0.0, 14.0),
        "volume_anomaly" => ((value.abs() / 5.0) * 16.0).clamp(0.0, 16.0),
        "base_breakout" => ((value.abs() / 10.0) * 14.0).clamp(0.0, 14.0),
        "estimate_revision_velocity" => ((value.abs() / 5.0) * 18.0).clamp(0.0, 18.0),
        "news_sentiment_shift" => ((value.abs() / 1.0) * 16.0).clamp(0.0, 16.0),
        _ => value.abs().min(10.0),
    }
}

fn list_fit_score(proposed_lists: &Value) -> f64 {
    let Some(lists) = proposed_lists.as_array() else {
        return 0.0;
    };
    lists
        .iter()
        .filter_map(|v| v.get("confidence").and_then(Value::as_str))
        .map(|confidence| match confidence {
            "high" => 10.0,
            "medium" => 5.0,
            "low" => 1.0,
            _ => 0.0,
        })
        .fold(0.0, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_revision_with_strong_fit_ranks_highest() {
        let got = rank_candidate(
            "estimate_revision_velocity",
            Some(4.0),
            Some(92.0),
            None,
            1,
            &serde_json::json!([{"confidence": "high"}]),
            false,
        );

        assert_eq!(got.bucket, "highest");
        assert!(got.score >= 75.0);
        assert!(got.reasons.contains(&"estimate revisions".to_string()));
        assert!(
            got.reasons
                .contains(&"high-confidence watchlist fit".to_string())
        );
    }

    #[test]
    fn weak_volume_only_candidate_stays_low_priority() {
        let got = rank_candidate(
            "volume_anomaly",
            Some(1.5),
            Some(40.0),
            None,
            3,
            &serde_json::json!([]),
            false,
        );

        assert_eq!(got.bucket, "low");
        assert!(got.score < 40.0);
    }

    #[test]
    fn research_nomination_can_rank_without_numeric_signal() {
        let got = rank_candidate(
            "research_nomination",
            None,
            Some(85.0),
            None,
            2,
            &serde_json::json!([{"confidence": "medium"}]),
            true,
        );

        assert_eq!(got.bucket, "high");
        assert!(got.reasons.contains(&"research nomination".to_string()));
        assert!(got.reasons.contains(&"suggests new watchlist".to_string()));
    }

    #[test]
    fn research_nomination_uses_source_count_and_theme_fit() {
        let strong = rank_candidate(
            "research_nomination",
            Some(4.0),
            Some(88.0),
            None,
            1,
            &serde_json::json!([]),
            false,
        );
        let weak = rank_candidate(
            "research_nomination",
            Some(2.0),
            Some(35.0),
            None,
            3,
            &serde_json::json!([]),
            false,
        );

        assert_eq!(strong.bucket, "high");
        assert!(strong.score > weak.score + 20.0);
        assert!(strong.reasons.contains(&"evidence ready".to_string()));
        assert!(strong.reasons.contains(&"strong theme fit 88".to_string()));
    }

    #[test]
    fn active_parent_theme_fit_boosts_rank() {
        let got = rank_candidate(
            "research_nomination",
            Some(3.0),
            Some(55.0),
            Some(82.0),
            2,
            &serde_json::json!([]),
            false,
        );

        assert_eq!(got.bucket, "high");
        assert!(
            got.reasons
                .contains(&"active parent theme fit 82".to_string())
        );
    }
}
