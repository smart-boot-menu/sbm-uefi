pub fn log_text<'a>(serial: &'a str, debugcon: &'a str) -> &'a str {
    if serial.contains("SBM_MARK") {
        serial
    } else {
        debugcon
    }
}

pub fn require_targets(log: &str) -> Result<(), String> {
    for name in [
        "Windows",
        "Ubuntu",
        "systemd-boot",
        "Fedora Linux",
        "SBM Core Slot A",
    ] {
        let line = format!("SBM_MARK target {name}");
        if !log.lines().any(|item| item.trim() == line) {
            return Err(format!("missing {line}"));
        }
    }
    if log.lines().any(|item| item.trim().ends_with("target sbm")) {
        return Err(String::from("the menu listed itself as target sbm"));
    }
    Ok(())
}

pub fn metric(log: &str, mark: &str) -> Option<u64> {
    for line in log.lines() {
        let Some(rest) = line.trim().strip_prefix("SBM_MARK ") else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(mark) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(value) = rest.strip_prefix("us=") else {
            continue;
        };
        let digits = value
            .split(|ch: char| !ch.is_ascii_digit())
            .next()
            .unwrap_or("");
        if let Ok(parsed) = digits.parse() {
            return Some(parsed);
        }
    }
    None
}

pub fn count_exact(log: &str, mark: &str) -> usize {
    let expected = format!("SBM_MARK {mark}");
    log.lines().filter(|line| line.trim() == expected).count()
}

pub fn count_prefix(log: &str, prefix: &str) -> usize {
    let expected = format!("SBM_MARK {prefix}");
    log.lines()
        .filter(|line| line.trim().starts_with(expected.as_str()))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
SBM_MARK uefi_entry
SBM_MARK first_frame us=1200
SBM_MARK discovery_done us=8000 count=5
SBM_MARK target Windows
SBM_MARK target Ubuntu
SBM_MARK target systemd-boot
SBM_MARK target Fedora Linux
SBM_MARK target SBM Core Slot A
SBM_MARK menu_ready us=15000
SBM_MARK fixture_started
noise SBM_MARK target sbm
";

    #[test]
    fn requires_the_fixture_targets() {
        let clean = LOG
            .lines()
            .filter(|line| !line.contains("target sbm\n") && *line != "noise SBM_MARK target sbm")
            .collect::<Vec<_>>()
            .join("\n");
        require_targets(&clean).unwrap();
        assert!(require_targets(LOG).is_err());
    }

    #[test]
    fn reads_microsecond_fields() {
        assert_eq!(metric(LOG, "first_frame"), Some(1200));
        assert_eq!(metric(LOG, "discovery_done"), Some(8000));
        assert_eq!(metric(LOG, "menu_ready"), Some(15000));
        assert_eq!(metric(LOG, "missing"), None);
    }

    #[test]
    fn prefers_serial_when_it_has_marks() {
        assert_eq!(
            log_text("SBM_MARK menu_ready us=1\n", "debug"),
            "SBM_MARK menu_ready us=1\n"
        );
        assert_eq!(
            log_text("firmware", "SBM_MARK menu_ready us=2\n"),
            "SBM_MARK menu_ready us=2\n"
        );
    }

    #[test]
    fn counts_exact_and_prefix_marks() {
        let log = "SBM_MARK fixture_started\nSBM_MARK chainload_returned status=SUCCESS\nSBM_MARK fixture_started\n";
        assert_eq!(count_exact(log, "fixture_started"), 2);
        assert_eq!(count_prefix(log, "chainload_returned"), 1);
    }
}
