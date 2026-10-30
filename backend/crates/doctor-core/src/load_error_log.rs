//! Parses the text of one load's error log, as StarRocks returns it from
//! `information_schema.load_tracking_logs` (identical to the ErrorURL file).
//! Each entry is `Error: <reason>. Row: <row>`; JSON errors run over several
//! lines and carry BE source locations, which are dropped. Categories come
//! from the reason text the BE writes (3.5.14 `RuntimeState` and scanners);
//! anything unrecognised is `Other` with the reason kept verbatim.
use std::collections::BTreeMap;

use doctor_contracts::{
    LOAD_ERROR_MAX_ENTRIES, LOAD_ERROR_REASON_MAX_BYTES, LOAD_ERROR_ROW_MAX_BYTES,
    LoadErrorCategory, LoadErrorCategoryCount, LoadErrorEntry, LoadErrorLogState,
};

/// Text beyond this is not parsed (the BE writes about 50 rows per file).
pub const LOAD_ERROR_LOG_MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedLoadErrorLog {
    pub state: LoadErrorLogState,
    pub entries: Vec<LoadErrorEntry>,
    pub categories: Vec<LoadErrorCategoryCount>,
    pub truncated: bool,
}

fn cut(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_owned(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

/// The BE already deleted the file.
fn is_missing_file_line(line: &str) -> bool {
    line.starts_with("Unable to canonicalize")
}

/// The scanner could not fetch the file (timeout, BE unreachable): not
/// proof that it is gone.
fn is_access_failure_line(line: &str) -> bool {
    line.starts_with("Failed to access ")
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    let rest = &text[from..];
    let to = rest.find(end)?;
    Some(rest[..to].trim()).filter(|value| !value.is_empty())
}

fn classify(reason: &str) -> (LoadErrorCategory, Option<String>, Option<String>) {
    if reason.contains("is out of range") {
        let column = between(reason, "(name = ", ",").map(str::to_owned);
        let target = between(reason, "Type: ", ",").map(str::to_owned);
        let textual = target.as_deref().is_some_and(|value| {
            let upper = value.to_ascii_uppercase();
            upper.starts_with("VARCHAR") || upper.starts_with("CHAR") || upper.starts_with("STRING")
        });
        let category = if textual {
            LoadErrorCategory::TooLong
        } else {
            LoadErrorCategory::OutOfRange
        };
        return (category, column, target);
    }
    if reason.contains("doesn't match source value column count") {
        return (LoadErrorCategory::ColumnCount, None, None);
    }
    if reason.contains("NULL value in non-nullable column") {
        return (
            LoadErrorCategory::NullInNonNullable,
            between(reason, "column '", "'").map(str::to_owned),
            None,
        );
    }
    if reason.contains("out of partition ranges") {
        return (LoadErrorCategory::OutOfPartition, None, None);
    }
    if reason.contains("Unable to cast") {
        let column = reason
            .split("column=")
            .nth(1)
            .map(|rest| {
                rest.split([',', ' ', '\n'])
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_owned()
            })
            .filter(|value| !value.is_empty());
        let target = reason
            .split(" value to ")
            .nth(1)
            .map(|rest| {
                rest.split(['.', ' ', ','])
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_owned()
            })
            .filter(|value| !value.is_empty());
        return (LoadErrorCategory::JsonCast, column, target);
    }
    if reason.contains("TAPE_ERROR")
        || reason.contains("improper structure")
        || reason.contains("JSON document")
    {
        return (LoadErrorCategory::JsonStructure, None, None);
    }
    (LoadErrorCategory::Other, None, None)
}

fn entry(index: usize, raw: &str) -> LoadErrorEntry {
    let body = raw.strip_prefix("Error: ").unwrap_or(raw);
    let (reason_part, row) = match body.find(". Row: ") {
        Some(at) => (&body[..at], Some(&body[at + ". Row: ".len()..])),
        None => (body, None),
    };
    // Drop BE source locations (`be/src/...:<line> <call>`) from the reason.
    let reason: Vec<&str> = reason_part
        .lines()
        .filter(|line| !line.trim_start().starts_with("be/src/"))
        .collect();
    let reason = reason.join("\n");
    let (category, column, target_type) = classify(&reason);
    let (reason, reason_cut) = cut(reason.trim(), LOAD_ERROR_REASON_MAX_BYTES);
    let row_converted = matches!(
        category,
        LoadErrorCategory::NullInNonNullable | LoadErrorCategory::OutOfPartition
    ) && row.is_some_and(|row| row.trim_start().starts_with('['));
    let (row, row_cut) = match row {
        Some(row) => {
            let (row, cutoff) = cut(row.trim_end_matches(['\r', '\n']), LOAD_ERROR_ROW_MAX_BYTES);
            (Some(row), cutoff)
        }
        None => (None, false),
    };
    LoadErrorEntry {
        index: u16::try_from(index + 1).unwrap_or(u16::MAX),
        category,
        column,
        target_type,
        reason,
        row,
        row_converted,
        truncated: reason_cut || row_cut,
    }
}

pub fn parse_load_error_log(text: Option<&str>) -> ParsedLoadErrorLog {
    let none = |state| ParsedLoadErrorLog {
        state,
        entries: Vec::new(),
        categories: Vec::new(),
        truncated: false,
    };
    let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
        return none(LoadErrorLogState::None);
    };
    let (text, mut truncated) = cut(text, LOAD_ERROR_LOG_MAX_BYTES);
    let mut raw_entries: Vec<String> = Vec::new();
    let mut missing = false;
    let mut unreachable = false;
    for line in text.lines() {
        if is_missing_file_line(line) {
            missing = true;
        } else if is_access_failure_line(line) {
            unreachable = true;
        } else if line.starts_with("Error: ") {
            raw_entries.push(line.to_owned());
        } else if let Some(current) = raw_entries.last_mut() {
            current.push('\n');
            current.push_str(line);
        }
    }
    if raw_entries.is_empty() {
        return none(if unreachable {
            LoadErrorLogState::Unavailable
        } else if missing {
            LoadErrorLogState::Purged
        } else {
            LoadErrorLogState::None
        });
    }
    if raw_entries.len() > LOAD_ERROR_MAX_ENTRIES {
        raw_entries.truncate(LOAD_ERROR_MAX_ENTRIES);
        truncated = true;
    }
    let entries: Vec<LoadErrorEntry> = raw_entries
        .iter()
        .enumerate()
        .map(|(index, raw)| entry(index, raw))
        .collect();
    let mut counts: BTreeMap<(LoadErrorCategory, Option<String>, Option<String>), u16> =
        BTreeMap::new();
    for item in &entries {
        *counts
            .entry((item.category, item.column.clone(), item.target_type.clone()))
            .or_default() += 1;
    }
    let mut categories: Vec<LoadErrorCategoryCount> = counts
        .into_iter()
        .map(
            |((category, column, target_type), count)| LoadErrorCategoryCount {
                category,
                column,
                target_type,
                count,
            },
        )
        .collect();
    categories.sort_by(|a, b| b.count.cmp(&a.count).then(a.category.cmp(&b.category)));
    ParsedLoadErrorLog {
        state: LoadErrorLogState::Available,
        entries,
        categories,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from official 3.5.14 (task 50): mixed CSV errors.
    const MIXED: &str = "Error: The field (name = id, pos = 0) is out of range. Type: INT, Value length: 1, Value: x. Row: x\tbadint\t2\t2026-01-05\n\
Error: The field (name = name, pos = 1) is out of range. Type: VARCHAR(5), Value length: 11, Value: toolongname. Row: 3\ttoolongname\t3\t2026-01-05\n\
Error: Target column count: 4 doesn't match source value column count: 3. Column separator: '\\t', Row delimiter: '\\n'. Row: 6\tfew\t6\n\
Error: NULL value in non-nullable column 'v'. Row: [4, 'nul', NULL, 2026-01-05]\n\
Error: The row is out of partition ranges. Please add a new partition.. Row: [5, 'part', 5, 2027-01-01]\n";

    /// JSON errors span lines and carry BE source locations.
    const JSON: &str = "Error: Invalid argument: Unable to cast string value to BIGINT. value=x, column=id\n\
be/src/formats/json/nullable_column.cpp:41 add_numeric_column<T>(data_column.get(), type_desc, name, value)\n\
be/src/exec/json_scanner.cpp:595 _construct_column(val, column.get(), _prev_parsed_position[key_index].type, _prev_parsed_position[key_index].key). Row: {\"id\":\"x\",\"doc\":{\"a\":2}}\n\
Error: Data quality error: construct row in object order failed, error: TAPE_ERROR: The JSON document has an improper structure: missing or superfluous commas, braces, missing keys, etc.. Row: {\"id\":3,\"doc\":\"not json\"]\n";

    #[test]
    fn ld03_csv_errors_are_categorised_with_column_and_type() {
        let parsed = parse_load_error_log(Some(MIXED));
        assert_eq!(parsed.state, LoadErrorLogState::Available);
        let kinds: Vec<_> = parsed.entries.iter().map(|e| e.category).collect();
        assert_eq!(
            kinds,
            [
                LoadErrorCategory::OutOfRange,
                LoadErrorCategory::TooLong,
                LoadErrorCategory::ColumnCount,
                LoadErrorCategory::NullInNonNullable,
                LoadErrorCategory::OutOfPartition,
            ]
        );
        let first = &parsed.entries[0];
        assert_eq!(
            (first.column.as_deref(), first.target_type.as_deref()),
            (Some("id"), Some("INT"))
        );
        assert_eq!(first.row.as_deref(), Some("x\tbadint\t2\t2026-01-05"));
        assert_eq!(
            first.reason,
            "The field (name = id, pos = 0) is out of range. Type: INT, Value length: 1, Value: x"
        );
        assert_eq!(parsed.entries[1].target_type.as_deref(), Some("VARCHAR(5)"));
        assert_eq!(parsed.entries[3].column.as_deref(), Some("v"));
        assert!(parsed.entries[3].row_converted && parsed.entries[4].row_converted);
        assert!(!parsed.entries[0].row_converted);
        assert_eq!(
            parsed.entries[4].row.as_deref(),
            Some("[5, 'part', 5, 2027-01-01]")
        );
        assert_eq!(parsed.categories.len(), 5);
    }

    #[test]
    fn ld03_json_errors_drop_source_locations_and_keep_the_row() {
        let parsed = parse_load_error_log(Some(JSON));
        assert_eq!(parsed.entries.len(), 2);
        let cast = &parsed.entries[0];
        assert_eq!(cast.category, LoadErrorCategory::JsonCast);
        assert_eq!(
            (cast.column.as_deref(), cast.target_type.as_deref()),
            (Some("id"), Some("BIGINT"))
        );
        assert!(!cast.reason.contains("be/src/"), "{}", cast.reason);
        assert_eq!(
            cast.row.as_deref(),
            Some("{\"id\":\"x\",\"doc\":{\"a\":2}}")
        );
        assert_eq!(parsed.entries[1].category, LoadErrorCategory::JsonStructure);
    }

    #[test]
    fn ld03_purged_empty_and_unknown_logs() {
        let purged = "Unable to canonicalize /data/deploy/starrocks/be/storage/error_log/error_log_594f: No such file or directory";
        assert_eq!(
            parse_load_error_log(Some(purged)).state,
            LoadErrorLogState::Purged
        );
        assert_eq!(parse_load_error_log(None).state, LoadErrorLogState::None);
        // A timeout or unreachable BE is not proof the file is gone.
        assert_eq!(
            parse_load_error_log(Some(
                "Failed to access http://127.0.0.1:8040/api/_load_error_log?file=x: timeout"
            ))
            .state,
            LoadErrorLogState::Unavailable
        );
        assert_eq!(
            parse_load_error_log(Some("  \n")).state,
            LoadErrorLogState::None
        );
        let other = parse_load_error_log(Some("Error: something new happened"));
        assert_eq!(other.entries[0].category, LoadErrorCategory::Other);
        assert_eq!(other.entries[0].row, None);
        // One file gone, another still there: the remaining entries show.
        let mixed = format!("{purged}\n{MIXED}");
        assert_eq!(parse_load_error_log(Some(&mixed)).entries.len(), 5);
    }

    #[test]
    fn ld03_long_rows_and_many_entries_are_capped() {
        let long = format!(
            "Error: The field (name = id, pos = 0) is out of range. Type: INT, Value length: 1, Value: y. Row: y\t{}\t1",
            "z".repeat(20_000)
        );
        let parsed = parse_load_error_log(Some(&long));
        let row = parsed.entries[0].row.as_deref().unwrap();
        assert!(row.len() <= LOAD_ERROR_ROW_MAX_BYTES && parsed.entries[0].truncated);
        let many: String = (0..250).map(|i| format!("Error: The field (name = id, pos = 0) is out of range. Type: INT, Value length: 1, Value: b{i}. Row: b{i}\n")).collect();
        let parsed = parse_load_error_log(Some(&many));
        assert_eq!(parsed.entries.len(), LOAD_ERROR_MAX_ENTRIES);
        assert!(parsed.truncated);
        assert_eq!(parsed.categories[0].count as usize, LOAD_ERROR_MAX_ENTRIES);
        let wide = format!("Error: x. Row: {}", "中".repeat(3_000));
        assert!(
            parse_load_error_log(Some(&wide)).entries[0]
                .row
                .as_deref()
                .unwrap()
                .len()
                <= LOAD_ERROR_ROW_MAX_BYTES
        );
    }
}
