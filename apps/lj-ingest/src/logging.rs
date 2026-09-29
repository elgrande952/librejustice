use tracing::Level;

fn parse_log_level(value: Option<&str>) -> Level {
    let Some(raw) = value else {
        return Level::INFO;
    };
    let raw = raw.trim().to_ascii_lowercase();
    if raw.is_empty() {
        return Level::INFO;
    }
    match raw.as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warning" | "warn" => Level::WARN,
        "error" => Level::ERROR,
        "critical" => Level::ERROR,
        other => match other.parse::<i32>() {
            Ok(n) => level_from_int(n),
            Err(_) => Level::INFO,
        },
    }
}

fn level_from_int(n: i32) -> Level {
    if n >= 40 {
        Level::ERROR
    } else if n >= 30 {
        Level::WARN
    } else if n >= 20 {
        Level::INFO
    } else if n >= 10 {
        Level::DEBUG
    } else {
        Level::TRACE
    }
}

pub fn resolve_level(level: Option<&str>) -> Level {
    match level {
        Some(value) => parse_log_level(Some(value)),
        None => parse_log_level(std::env::var("LIBREJUSTICE_LOG_LEVEL").ok().as_deref()),
    }
}

pub fn json_format() -> bool {
    matches!(std::env::var("LOG_FORMAT"), Ok(v) if v == "json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::Level;

    #[test]
    fn parse_named_levels() {
        assert_eq!(parse_log_level(Some("trace")), Level::TRACE);
        assert_eq!(parse_log_level(Some("DEBUG")), Level::DEBUG);
        assert_eq!(parse_log_level(Some(" Info ")), Level::INFO);
        assert_eq!(parse_log_level(Some("warning")), Level::WARN);
        assert_eq!(parse_log_level(Some("error")), Level::ERROR);
        assert_eq!(parse_log_level(Some("critical")), Level::ERROR);
    }

    #[test]
    fn parse_default_and_unknown() {
        assert_eq!(parse_log_level(None), Level::INFO);
        assert_eq!(parse_log_level(Some("")), Level::INFO);
        assert_eq!(parse_log_level(Some("nonsense")), Level::INFO);
    }

    #[test]
    fn parse_int_levels() {
        assert_eq!(parse_log_level(Some("5")), Level::TRACE);
        assert_eq!(parse_log_level(Some("10")), Level::DEBUG);
        assert_eq!(parse_log_level(Some("20")), Level::INFO);
        assert_eq!(parse_log_level(Some("30")), Level::WARN);
        assert_eq!(parse_log_level(Some("50")), Level::ERROR);
    }
}
