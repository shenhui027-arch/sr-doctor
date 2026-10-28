//! The only product SQL normalizer. It fingerprints audit statements that
//! carry no native digest. Clean-room: lexical rules follow the fixed-version
//! StarRocks 3.5.14 grammar (`StarRocksLex.g4`, `StarRocks.g4` @23a56ec), and
//! every equivalence it applies was observed as one official digest class on
//! the fixed-digest image (fixtures/oracle/sql-fingerprint). Where the
//! official digest depends on analysis this token-level normalizer cannot
//! reproduce, it keeps statements apart (over-split) or reports them
//! unavailable; it never merges what the official digest separates.
use doctor_contracts::{
    PRODUCT_SQL_FINGERPRINT_VERSION, ProductSqlFingerprint, ProductSqlFingerprintPartialReason,
    ProductSqlFingerprintUnavailableReason as Unavailable,
};
use sha2::{Digest, Sha256};

pub struct SqlFingerprintInput<'a> {
    /// `None` is a SQL NULL `stmt`.
    pub statement: Option<&'a str>,
    /// Only a prefix of the audited statement was read.
    pub truncated: bool,
    /// The execution's session catalog and database, for qualifier folding.
    pub catalog: Option<&'a str>,
    pub database: Option<&'a str>,
}

pub fn product_sql_fingerprint(input: SqlFingerprintInput<'_>) -> ProductSqlFingerprint {
    let Some(statement) = input.statement else {
        return ProductSqlFingerprint::Unavailable {
            reason: Unavailable::StatementSqlNull,
        };
    };
    if input.truncated {
        return ProductSqlFingerprint::Partial {
            reason: ProductSqlFingerprintPartialReason::StatementTruncated,
        };
    }
    match normalize(statement, input.catalog, input.database) {
        Ok(template) => {
            let mut hasher = Sha256::new();
            hasher.update(PRODUCT_SQL_FINGERPRINT_VERSION.as_bytes());
            hasher.update([0]);
            hasher.update(template.as_bytes());
            ProductSqlFingerprint::Exact {
                fingerprint: format!("{:x}", hasher.finalize()),
                template,
            }
        }
        Err(reason) => ProductSqlFingerprint::Unavailable { reason },
    }
}

/// Reserved words of the fixed-version grammar: lexer keyword tokens minus
/// the parser's `nonReserved` rule. Only these can never be identifiers, so
/// only these are case-folded unconditionally.
const RESERVED: &[&str] = &[
    "ADD",
    "ALL",
    "ALTER",
    "ANALYZE",
    "AND",
    "ARRAY",
    "AS",
    "ASC",
    "BETWEEN",
    "BIGINT",
    "BITMAP",
    "BIT_SHIFT_LEFT",
    "BIT_SHIFT_RIGHT",
    "BIT_SHIFT_RIGHT_LOGICAL",
    "BOTH",
    "BY",
    "CASE",
    "CHAR",
    "CHARACTER",
    "CHECK",
    "COLLATE",
    "COLUMN",
    "COMPACTION",
    "CONVERT",
    "CREATE",
    "CROSS",
    "CUBE",
    "CURRENT_DATE",
    "CURRENT_GROUP",
    "CURRENT_ROLE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "DATABASE",
    "DATABASES",
    "DECIMAL",
    "DECIMAL128",
    "DECIMAL32",
    "DECIMAL64",
    "DECIMALV2",
    "DEFAULT",
    "DEFERRED",
    "DELETE",
    "DENSE_RANK",
    "DESC",
    "DESCRIBE",
    "DISTINCT",
    "DOUBLE",
    "DROP",
    "DUAL",
    "ELSE",
    "EXCEPT",
    "EXISTS",
    "EXPLAIN",
    "FALSE",
    "FILES",
    "FIRST_VALUE",
    "FLOAT",
    "FOR",
    "FORCE",
    "FROM",
    "FULL",
    "FUNCTION",
    "GIN",
    "GRANT",
    "GROUP",
    "GROUPING",
    "GROUPING_ID",
    "GROUPS",
    "HAVING",
    "HLL",
    "IF",
    "IGNORE",
    "IMMEDIATE",
    "IN",
    "INDEX",
    "INFILE",
    "INNER",
    "INSERT",
    "INT",
    "INTEGER",
    "INTERSECT",
    "INTO",
    "INT_DIV",
    "INVOKER",
    "IS",
    "JOIN",
    "JSON",
    "KEY",
    "KEYS",
    "KILL",
    "LAG",
    "LARGEINT",
    "LAST_VALUE",
    "LATERAL",
    "LEAD",
    "LEFT",
    "LIKE",
    "LIMIT",
    "LOAD",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "MAXVALUE",
    "MICROSECOND",
    "MILLISECOND",
    "MOD",
    "NOT",
    "NTILE",
    "NULL",
    "ON",
    "OPTIMIZE",
    "OR",
    "ORDER",
    "OUTER",
    "OUTFILE",
    "OVER",
    "PARTITION",
    "PERCENTILE",
    "PREPARE",
    "PRIMARY",
    "PROCEDURE",
    "RANGE",
    "READ",
    "REGEXP",
    "RELEASE",
    "RENAME",
    "REPLACE",
    "REVOKE",
    "RIGHT",
    "RLIKE",
    "ROWS",
    "ROW_NUMBER",
    "SCHEMA",
    "SCHEMAS",
    "SELECT",
    "SET",
    "SET_VAR",
    "SHOW",
    "SMALLINT",
    "SYSTEM",
    "TABLE",
    "TERMINATED",
    "TEXT",
    "THEN",
    "TINYINT",
    "TO",
    "TRUE",
    "UNION",
    "UNIQUE",
    "UNSIGNED",
    "UPDATE",
    "USE",
    "USING",
    "VALUES",
    "VARCHAR",
    "WHEN",
    "WHERE",
    "WITH",
];

const INTERVAL_UNITS: &[&str] = &[
    "YEAR",
    "MONTH",
    "WEEK",
    "DAY",
    "HOUR",
    "MINUTE",
    "SECOND",
    "QUARTER",
    "MILLISECOND",
    "MICROSECOND",
];

/// Longest-first, so a prefix never shadows a longer operator.
const OPERATORS: &[&str] = &[
    "<=>", "[*]", "...", "<=", ">=", "<>", "!=", "||", "&&", "->", "=", "<", ">", "+", "-", "*",
    "/", "%", "!", "&", "|", "^", "~", "@", "(", ")", "[", "]", "{", "}", ",", ".", ":",
];

/// An official CompoundPredicate with this many flattened terms prints as its
/// column set; 15 AND/OR operators can already form one.
const MASSIVE_COMPOUND_OPERATORS: usize = 15;

#[derive(Clone, Debug, PartialEq)]
enum Tok<'a> {
    Word(&'a str),
    Quoted(String),
    Number(&'a str),
    Str,
    Binary,
    Param,
    Hint(&'a str),
    Op(&'static str),
    Semicolon,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Out {
    Kw(String),
    Ident(String),
    Function(String),
    Placeholder,
    /// An ORDER BY / GROUP BY position: its value selects a column.
    Ordinal(String),
    Op(&'static str),
    /// Type precision/scale/length selects a type, rather than a SQL literal.
    TypeParameter(String),
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\r' | '\n' | '\t' | '\u{3000}')
}

fn is_letter(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '$' || ('\u{80}'..='\u{ffff}').contains(&c)
}

fn is_ident_char(c: char) -> bool {
    is_letter(c) || c.is_ascii_digit()
}

fn digits(text: &str) -> usize {
    text.bytes().take_while(u8::is_ascii_digit).count()
}

/// Longest INTEGER / DECIMAL / DOUBLE match at `text`, or 0.
fn number_len(text: &str) -> usize {
    let mut len = digits(text);
    if text[len..].starts_with('.') {
        let fraction = digits(&text[len + 1..]);
        if len == 0 && fraction == 0 {
            return 0;
        }
        len += 1 + fraction;
    }
    if len == 0 {
        return 0;
    }
    let rest = &text[len..];
    if rest.starts_with(['e', 'E']) {
        let sign = usize::from(rest[1..].starts_with(['+', '-']));
        let exponent = digits(&rest[1 + sign..]);
        if exponent > 0 {
            len += 1 + sign + exponent;
        }
    }
    len
}

/// DIGIT_IDENTIFIER: a digit, then at least one identifier character.
fn digit_identifier_len(text: &str) -> usize {
    let len: usize = text
        .chars()
        .take_while(|c| is_ident_char(*c))
        .map(char::len_utf8)
        .sum();
    if len >= 2 { len } else { 0 }
}

fn quoted_len(text: &str, quote: char) -> Option<usize> {
    let mut chars = text.char_indices().skip(1);
    while let Some((at, c)) = chars.next() {
        if c == '\\' {
            chars.next()?;
        } else if c == quote {
            if text[at + 1..].starts_with(quote) {
                chars.next();
            } else {
                return Some(at + 1);
            }
        }
    }
    None
}

fn lex(text: &str) -> Result<Vec<Tok<'_>>, Unavailable> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while let Some(c) = text[at..].chars().next() {
        let rest = &text[at..];
        if is_ws(c) {
            at += c.len_utf8();
        } else if rest.starts_with("--") {
            at += rest.find('\n').map_or(rest.len(), |end| end + 1);
        } else if let Some(body) = rest.strip_prefix("/*") {
            let end = body.find("*/").ok_or(Unavailable::LexicalError)? + 4;
            if rest.starts_with("/*+") {
                tokens.push(Tok::Hint(&rest[..end]));
            }
            at += end;
        } else if c == '\'' || c == '"' {
            at += quoted_len(rest, c).ok_or(Unavailable::LexicalError)?;
            tokens.push(Tok::Str);
        } else if (c == 'x' || c == 'X') && rest[1..].starts_with(['\'', '"']) {
            let quote = &rest[1..2];
            let end = rest[2..]
                .find(|next: char| next == '\\' || quote.starts_with(next))
                .filter(|end| rest[2 + end..].starts_with(quote))
                .ok_or(Unavailable::LexicalError)?;
            at += end + 3;
            tokens.push(Tok::Binary);
        } else if c == '`' {
            let mut value = String::new();
            let mut chars = rest.char_indices().skip(1);
            let end = loop {
                let (index, next) = chars.next().ok_or(Unavailable::LexicalError)?;
                if next != '`' {
                    value.push(next);
                } else if rest[index + 1..].starts_with('`') {
                    value.push('`');
                    chars.next();
                } else {
                    break index + 1;
                }
            };
            at += end;
            tokens.push(Tok::Quoted(value));
        } else if c.is_ascii_digit() {
            let number = number_len(rest);
            let identifier = digit_identifier_len(rest);
            if identifier > number {
                tokens.push(Tok::Word(&rest[..identifier]));
                at += identifier;
            } else {
                tokens.push(Tok::Number(&rest[..number]));
                at += number;
            }
        } else if c == '.' && rest[1..].starts_with(|next: char| next.is_ascii_digit()) {
            let number = number_len(rest);
            let identifier = 1 + digit_identifier_len(&rest[1..]);
            if identifier > number && identifier > 1 {
                tokens.push(Tok::Op("."));
                tokens.push(Tok::Word(&rest[1..identifier]));
                at += identifier;
            } else {
                tokens.push(Tok::Number(&rest[..number]));
                at += number;
            }
        } else if rest.starts_with("$$") {
            // ATTACHMENT text has no stable boundary at token level.
            return Err(Unavailable::LexicalError);
        } else if is_letter(c) {
            let len: usize = rest
                .chars()
                .take_while(|next| is_ident_char(*next))
                .map(char::len_utf8)
                .sum();
            tokens.push(Tok::Word(&rest[..len]));
            at += len;
        } else if c == '?' {
            tokens.push(Tok::Param);
            at += 1;
        } else if c == ';' {
            tokens.push(Tok::Semicolon);
            at += 1;
        } else {
            let op = OPERATORS
                .iter()
                .find(|op| rest.starts_with(**op))
                .ok_or(Unavailable::LexicalError)?;
            tokens.push(Tok::Op(op));
            at += op.len();
        }
    }
    Ok(tokens)
}

fn word_is(token: Option<&Tok<'_>>, expected: &str) -> bool {
    matches!(token, Some(Tok::Word(word)) if word.eq_ignore_ascii_case(expected))
}

fn reserved(word: &str) -> Option<String> {
    let upper = word.to_ascii_uppercase();
    RESERVED.contains(&upper.as_str()).then_some(upper)
}

fn validate(tokens: &[Tok<'_>]) -> Result<(), Unavailable> {
    if tokens.is_empty() {
        return Err(Unavailable::EmptyStatement);
    }
    if tokens.contains(&Tok::Semicolon) {
        return Err(Unavailable::MultipleStatements);
    }
    let mut brackets = Vec::new();
    for token in tokens {
        match token {
            Tok::Op(open @ ("(" | "[" | "{")) => brackets.push(*open),
            Tok::Op(close @ (")" | "]" | "}")) => {
                let expected = match *close {
                    ")" => "(",
                    "]" => "[",
                    _ => "{",
                };
                if brackets.pop() != Some(expected) {
                    return Err(Unavailable::UnbalancedParentheses);
                }
            }
            _ => {}
        }
    }
    if !brackets.is_empty() {
        return Err(Unavailable::UnbalancedParentheses);
    }
    let first = tokens
        .iter()
        .find(|token| !matches!(token, Tok::Op("(") | Tok::Hint(_)));
    if !["SELECT", "WITH", "INSERT", "UPDATE", "DELETE"]
        .iter()
        .any(|kind| word_is(first, kind))
    {
        return Err(Unavailable::UnsupportedStatement);
    }
    // AND/OR operators, except the AND that closes a BETWEEN at its depth.
    let (mut operators, mut depth, mut betweens) = (0, 0usize, Vec::new());
    for token in tokens {
        match token {
            Tok::Op("(") => depth += 1,
            Tok::Op(")") => depth -= 1,
            Tok::Word(word) if word.eq_ignore_ascii_case("BETWEEN") => betweens.push(depth),
            Tok::Word(word) if word.eq_ignore_ascii_case("AND") => {
                if betweens.last() == Some(&depth) {
                    betweens.pop();
                } else {
                    operators += 1;
                }
            }
            Tok::Word(word) if word.eq_ignore_ascii_case("OR") => operators += 1,
            Tok::Op("&&") | Tok::Op("||") => operators += 1,
            _ => {}
        }
    }
    if operators >= MASSIVE_COMPOUND_OPERATORS {
        return Err(Unavailable::MassiveCompoundPredicate);
    }
    Ok(())
}

fn matching_close(tokens: &[Tok<'_>], open: usize) -> usize {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token {
            Tok::Op("(") => depth += 1,
            Tok::Op(")") => {
                depth -= 1;
                if depth == 0 {
                    return index;
                }
            }
            _ => {}
        }
    }
    tokens.len() - 1
}

/// A LIMIT operand: an integer literal or a parameter. `Some(true)` is zero.
fn limit_operand(token: Option<&Tok<'_>>) -> Option<bool> {
    match token? {
        Tok::Number(number) if number.bytes().all(|byte| byte.is_ascii_digit()) => {
            Some(number.bytes().all(|byte| byte == b'0'))
        }
        Tok::Param => Some(false),
        _ => None,
    }
}

/// A bare integer that is a whole ORDER BY / GROUP BY item is a position,
/// not a literal: the official digest keeps `ORDER BY 1` and `ORDER BY 2` apart.
fn ordinal(number: &str, last: Option<&Out>, next: Option<&Tok<'_>>) -> bool {
    let item_start = matches!(last, Some(Out::Kw(kw)) if kw == "BY") || last == Some(&Out::Op(","));
    let item_end = match next {
        None | Some(Tok::Op(",") | Tok::Op(")") | Tok::Hint(_)) => true,
        Some(Tok::Word(word)) => ![
            "AND", "OR", "DIV", "MOD", "IS", "IN", "NOT", "LIKE", "BETWEEN", "REGEXP", "RLIKE",
        ]
        .iter()
        .any(|operator| word.eq_ignore_ascii_case(operator)),
        _ => false,
    };
    item_start && item_end && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether a value ends right before this point, making `-` binary.
fn value_ends(last: Option<&Out>) -> bool {
    matches!(
        last,
        Some(Out::Ident(_) | Out::Function(_) | Out::Placeholder | Out::Op(")" | "]"))
    ) || matches!(last, Some(Out::Kw(kw)) if kw == "END")
}

fn normalize(
    text: &str,
    catalog: Option<&str>,
    database: Option<&str>,
) -> Result<String, Unavailable> {
    normalized_tokens(text, catalog, database).map(|out| render(&out))
}

/// Only the fixed grammar's scalar CAST types are recognized here. Complex
/// types fail closed until their full grammar is supported; a type parameter
/// must never be replaced with a value placeholder.
fn cast_parameters(tokens: &[Tok<'_>]) -> Result<std::collections::BTreeSet<usize>, Unavailable> {
    let mut parameters = std::collections::BTreeSet::new();
    let scalar = [
        "BOOLEAN",
        "TINYINT",
        "SMALLINT",
        "INT",
        "INTEGER",
        "BIGINT",
        "LARGEINT",
        "FLOAT",
        "DOUBLE",
        "DATE",
        "DATETIME",
        "TIME",
        "CHAR",
        "VARCHAR",
        "STRING",
        "TEXT",
        "BITMAP",
        "HLL",
        "PERCENTILE",
        "JSON",
        "VARBINARY",
        "BINARY",
        "DECIMAL",
        "DECIMALV2",
        "DECIMAL32",
        "DECIMAL64",
        "DECIMAL128",
        "NUMERIC",
        "NUMBER",
    ];
    let parameterized = [
        "TINYINT",
        "SMALLINT",
        "INT",
        "INTEGER",
        "BIGINT",
        "LARGEINT",
        "CHAR",
        "VARCHAR",
        "VARBINARY",
        "BINARY",
        "DECIMAL",
        "DECIMALV2",
        "DECIMAL32",
        "DECIMAL64",
        "DECIMAL128",
        "NUMERIC",
        "NUMBER",
    ];
    let mut accepted_types = std::collections::BTreeSet::new();
    for index in 0..tokens.len() {
        if !word_is(tokens.get(index), "CAST") || tokens.get(index + 1) != Some(&Tok::Op("(")) {
            continue;
        }
        let end = matching_close(tokens, index + 1);
        let mut depth = 0usize;
        let mut type_at = None;
        for (at, token) in tokens.iter().enumerate().take(end).skip(index + 2) {
            match token {
                Tok::Op("(") => depth += 1,
                Tok::Op(")") => depth = depth.saturating_sub(1),
                _ if depth == 0 && word_is(Some(token), "AS") => type_at = Some(at + 1),
                _ => {}
            }
        }
        let mut at = type_at.ok_or(Unavailable::UnsupportedStatement)?;
        if word_is(tokens.get(at), "SIGNED") || word_is(tokens.get(at), "UNSIGNED") {
            at += 1;
            if at == end {
                continue;
            }
            if !word_is(tokens.get(at), "INT") && !word_is(tokens.get(at), "INTEGER") {
                return Err(Unavailable::UnsupportedStatement);
            }
        }
        let Some(Tok::Word(kind)) = tokens.get(at) else {
            return Err(Unavailable::UnsupportedStatement);
        };
        let upper = kind.to_ascii_uppercase();
        if !scalar.contains(&upper.as_str()) {
            return Err(Unavailable::UnsupportedStatement);
        }
        accepted_types.insert(at);
        if at + 1 == end {
            continue;
        }
        if !parameterized.contains(&upper.as_str())
            || tokens.get(at + 1) != Some(&Tok::Op("("))
            || tokens.get(end - 1) != Some(&Tok::Op(")"))
        {
            return Err(Unavailable::UnsupportedStatement);
        }
        let values = &tokens[at + 2..end - 1];
        let integer = |token: &Tok<'_>| matches!(token, Tok::Number(value) if value.bytes().all(|b| b.is_ascii_digit()));
        let decimal = matches!(
            upper.as_str(),
            "DECIMAL"
                | "DECIMALV2"
                | "DECIMAL32"
                | "DECIMAL64"
                | "DECIMAL128"
                | "NUMERIC"
                | "NUMBER"
        );
        if !(values.len() == 1 && integer(&values[0])
            || decimal
                && values.len() == 3
                && integer(&values[0])
                && values[1] == Tok::Op(",")
                && integer(&values[2]))
        {
            return Err(Unavailable::UnsupportedStatement);
        }
        parameters.insert(at + 2);
        if values.len() == 3 {
            parameters.insert(at + 4);
        }
    }
    // A parameterized type in another syntax (e.g. CONVERT or ARRAY<...>)
    // needs its own grammar, never the generic literal-folding path.
    for (at, token) in tokens.iter().enumerate() {
        if let Tok::Word(word) = token
            && parameterized.contains(&word.to_ascii_uppercase().as_str())
            && tokens.get(at + 1) == Some(&Tok::Op("("))
            && !accepted_types.contains(&at)
        {
            return Err(Unavailable::UnsupportedStatement);
        }
    }
    Ok(parameters)
}

/// The normalized token stream the template is rendered from; A2 structure
/// extraction reads the same stream so both share one set of rules.
pub(crate) fn normalized_tokens(
    text: &str,
    catalog: Option<&str>,
    database: Option<&str>,
) -> Result<Vec<Out>, Unavailable> {
    let mut tokens = lex(text)?;
    while tokens.last() == Some(&Tok::Semicolon) {
        tokens.pop();
    }
    validate(&tokens)?;
    let type_parameters = cast_parameters(&tokens)?;
    let mut out: Vec<Out> = Vec::with_capacity(tokens.len());
    let (mut depth, mut cases, mut values, mut by_list) = (0usize, Vec::new(), None, None);
    let mut index = 0;
    while index < tokens.len() {
        let next = tokens.get(index + 1);
        match &tokens[index] {
            // SELECT hints never reach the official digest. Other positions
            // need a full hint grammar: refuse their template rather than
            // storing literals or changing exact identity by deleting a hint.
            Tok::Hint(_) => {
                if !matches!(out.last(), Some(Out::Kw(kw)) if kw == "SELECT") {
                    return Err(Unavailable::UnsupportedStatement);
                }
            }
            Tok::Number(number) if type_parameters.contains(&index) => {
                out.push(Out::TypeParameter((*number).to_owned()));
            }
            Tok::Number(number) if by_list == Some(depth) && ordinal(number, out.last(), next) => {
                out.push(Out::Ordinal((*number).to_owned()))
            }
            Tok::Str | Tok::Binary | Tok::Param | Tok::Number(_) => out.push(Out::Placeholder),
            Tok::Op("-") if matches!(next, Some(Tok::Number(_))) && !value_ends(out.last()) => {}
            Tok::Op("<>") => out.push(Out::Op("!=")),
            Tok::Op("(") => {
                depth += 1;
                out.push(Out::Op("("));
            }
            Tok::Op(")") => {
                depth -= 1;
                if by_list.is_some_and(|list| depth < list) {
                    by_list = None;
                }
                out.push(Out::Op(")"));
                // VALUES keeps only its first row.
                if values == Some(depth) {
                    values = None;
                    while tokens.get(index + 1) == Some(&Tok::Op(","))
                        && tokens.get(index + 2) == Some(&Tok::Op("("))
                    {
                        index = matching_close(&tokens, index + 2);
                    }
                }
            }
            Tok::Op(op) => out.push(Out::Op(op)),
            Tok::Quoted(name) => out.push(Out::Ident(name.clone())),
            Tok::Semicolon => unreachable!("validated"),
            Tok::Word(word) => {
                if let Some(kw) = reserved(word) {
                    if by_list == Some(depth)
                        && matches!(
                            kw.as_str(),
                            "HAVING" | "LIMIT" | "UNION" | "EXCEPT" | "INTERSECT" | "ORDER"
                        )
                    {
                        by_list = None;
                    }
                    match kw.as_str() {
                        "TRUE" | "FALSE" => out.push(Out::Placeholder),
                        "NULL" => {
                            let after_is = match out.as_slice() {
                                [.., Out::Kw(is), Out::Kw(not)] if not == "NOT" => is == "IS",
                                [.., Out::Kw(is)] => is == "IS",
                                _ => false,
                            };
                            out.push(if after_is {
                                Out::Kw(kw)
                            } else {
                                Out::Placeholder
                            });
                        }
                        "ASC" => {}
                        "INNER" if word_is(next, "JOIN") => {}
                        "CASE" => {
                            cases.push(depth);
                            out.push(Out::Kw(kw));
                        }
                        "VALUES" => {
                            if next == Some(&Tok::Op("(")) {
                                values = Some(depth);
                            }
                            out.push(Out::Kw(kw));
                        }
                        "LIMIT" => {
                            out.push(Out::Kw(kw));
                            if let Some(first_zero) = limit_operand(next) {
                                let separator = tokens.get(index + 2);
                                let second = limit_operand(tokens.get(index + 3));
                                // LIMIT offset, count  |  LIMIT count OFFSET offset
                                let offset_zero = match (separator, second) {
                                    (Some(Tok::Op(",")), Some(_)) => Some(first_zero),
                                    (separator, Some(zero)) if word_is(separator, "OFFSET") => {
                                        Some(zero)
                                    }
                                    _ => None,
                                };
                                out.push(Out::Placeholder);
                                index += 1;
                                if let Some(offset_zero) = offset_zero {
                                    if !offset_zero {
                                        out.extend([Out::Op(","), Out::Placeholder]);
                                    }
                                    index += 2;
                                }
                            }
                        }
                        "BY" => {
                            if matches!(out.last(), Some(Out::Kw(kw)) if kw == "ORDER" || kw == "GROUP")
                            {
                                by_list = Some(depth);
                            }
                            out.push(Out::Kw(kw));
                        }
                        _ => out.push(Out::Kw(kw)),
                    }
                } else {
                    let upper = word.to_ascii_uppercase();
                    match upper.as_str() {
                        "END" if cases.last() == Some(&depth) => {
                            cases.pop();
                            out.push(Out::Kw(upper));
                        }
                        "DATE" | "DATETIME" if next == Some(&Tok::Str) => {
                            out.push(Out::Placeholder);
                            index += 1;
                        }
                        "INTERVAL" => {
                            let span = match (next, tokens.get(index + 2)) {
                                (Some(Tok::Number(_) | Tok::Str | Tok::Param), _) => 1,
                                (Some(Tok::Op("-")), Some(Tok::Number(_))) => 2,
                                _ => 0,
                            };
                            match tokens.get(index + 1 + span) {
                                Some(Tok::Word(unit))
                                    if span > 0
                                        && INTERVAL_UNITS
                                            .contains(&unit.to_ascii_uppercase().as_str()) =>
                                {
                                    // The official printer keeps the unit as written.
                                    out.extend([
                                        Out::Kw(upper),
                                        Out::Placeholder,
                                        Out::Ident((*unit).to_owned()),
                                    ]);
                                    index += 1 + span;
                                }
                                _ => out.push(Out::Ident((*word).to_owned())),
                            }
                        }
                        "LABEL"
                            if matches!(out.last(), Some(Out::Kw(kw)) if kw == "WITH")
                                && matches!(next, Some(Tok::Word(_) | Tok::Quoted(_))) =>
                        {
                            out.extend([Out::Kw(upper), Out::Placeholder]);
                            index += 1;
                        }
                        "NULLS" if word_is(next, "FIRST") || word_is(next, "LAST") => {
                            let Some(Tok::Word(position)) = next else {
                                unreachable!()
                            };
                            out.extend([Out::Kw(upper), Out::Kw(position.to_ascii_uppercase())]);
                            index += 1;
                        }
                        "SEMI" | "ANTI" if word_is(next, "JOIN") => out.push(Out::Kw(upper)),
                        _ if next == Some(&Tok::Op("("))
                            && function_position(&out, &tokens, index) =>
                        {
                            out.push(Out::Function(word.to_ascii_lowercase()))
                        }
                        _ => out.push(Out::Ident((*word).to_owned())),
                    }
                }
            }
        }
        index += 1;
    }
    collapse_constant_in_lists(&mut out);
    fold_session_qualifiers(&mut out, catalog, database);
    Ok(out)
}

/// A word before `(` is a function name unless it names a table (INSERT
/// target, `TABLE`, a qualified name), an alias, or a CTE column list.
fn function_position(out: &[Out], tokens: &[Tok<'_>], index: usize) -> bool {
    let target = match out {
        [.., Out::Op("." | ")")] => true,
        [.., Out::Kw(kw)] => matches!(kw.as_str(), "INTO" | "TABLE" | "AS"),
        [.., Out::Kw(insert), Out::Ident(overwrite)] => {
            insert == "INSERT" && overwrite.eq_ignore_ascii_case("OVERWRITE")
        }
        _ => false,
    };
    let close = matching_close(tokens, index + 1);
    let cte_columns =
        word_is(tokens.get(close + 1), "AS") && tokens.get(close + 2) == Some(&Tok::Op("("));
    !target && !cte_columns
}

/// `IN (?, ?, ...)` of only literals is one official `IN (?)`.
fn collapse_constant_in_lists(out: &mut Vec<Out>) {
    let mut index = 0;
    while index + 1 < out.len() {
        if matches!(&out[index], Out::Kw(kw) if kw == "IN") && out[index + 1] == Out::Op("(") {
            let mut end = index + 2;
            let mut constant = true;
            loop {
                if out.get(end) != Some(&Out::Placeholder) {
                    constant = false;
                    break;
                }
                match out.get(end + 1) {
                    Some(Out::Op(",")) => end += 2,
                    Some(Out::Op(")")) => break,
                    _ => {
                        constant = false;
                        break;
                    }
                }
            }
            if constant {
                out.drain(index + 3..end + 1);
            }
        }
        index += 1;
    }
}

/// The official digest prints analysed table names, so `t`, `db.t` and
/// `catalog.db.t` are one class in the session's own catalog and database.
/// Only names right after FROM / JOIN / INTO / UPDATE are folded.
fn fold_session_qualifiers(out: &mut Vec<Out>, catalog: Option<&str>, database: Option<&str>) {
    let Some(database) = database else { return };
    let ident =
        |item: Option<&Out>, name: &str| matches!(item, Some(Out::Ident(value)) if value == name);
    let mut index = 0;
    while index < out.len() {
        if matches!(&out[index], Out::Kw(kw) if matches!(kw.as_str(), "FROM" | "JOIN" | "INTO" | "UPDATE"))
        {
            let start = index + 1;
            let three_part = catalog.is_some_and(|catalog| ident(out.get(start), catalog))
                && out.get(start + 1) == Some(&Out::Op("."))
                && ident(out.get(start + 2), database)
                && out.get(start + 3) == Some(&Out::Op("."))
                && matches!(out.get(start + 4), Some(Out::Ident(_)))
                && out.get(start + 5) != Some(&Out::Op("."));
            let two_part = ident(out.get(start), database)
                && out.get(start + 1) == Some(&Out::Op("."))
                && matches!(out.get(start + 2), Some(Out::Ident(_)))
                && out.get(start + 3) != Some(&Out::Op("."));
            if three_part {
                out.drain(start..start + 4);
            } else if two_part {
                out.drain(start..start + 2);
            }
        }
        index += 1;
    }
}

fn plain_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(is_letter) && chars.all(is_ident_char) && reserved(name).is_none()
}

fn render(out: &[Out]) -> String {
    let mut text = String::new();
    let mut previous: Option<&Out> = None;
    for item in out {
        let tight = matches!(previous, None | Some(Out::Op("(" | "." | "[" | "@")))
            || matches!(item, Out::Op(")" | "," | "." | "]"))
            || (matches!(item, Out::Op("(")) && matches!(previous, Some(Out::Function(_))));
        if !tight {
            text.push(' ');
        }
        match item {
            Out::Kw(kw) => text.push_str(kw),
            Out::Function(name) => text.push_str(name),
            Out::Ident(name) if plain_identifier(name) => text.push_str(name),
            Out::Ident(name) => {
                text.push('`');
                text.push_str(&name.replace('`', "``"));
                text.push('`');
            }
            Out::Placeholder => text.push('?'),
            Out::Ordinal(position) => text.push_str(position),
            Out::Op(op) => text.push_str(op),
            Out::TypeParameter(parameter) => text.push_str(parameter),
        }
        previous = Some(item);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_v1_07_mismatched_delimiters_are_unavailable_without_panics() {
        for text in ["SELECT [1)", "SELECT ([1)]", "SELECT (1]", "SELECT {1)"] {
            let result = std::panic::catch_unwind(|| template(text));
            assert!(result.is_ok(), "{text}: parser panicked");
            assert_eq!(
                result.unwrap(),
                Err(Unavailable::UnbalancedParentheses),
                "{text}"
            );
        }
        assert!(template("SELECT ARRAY[1,2]").is_ok());
        assert!(template("SELECT (ARRAY[1,2])[1]").is_ok());
    }

    #[test]
    fn audit_v1_06_cast_type_parameters_are_structure_not_literals() {
        let a = template("SELECT CAST(id AS DECIMAL(10, 2)) FROM t").unwrap();
        let b = template("SELECT CAST(id AS DECIMAL(20, 4)) FROM t").unwrap();
        assert_ne!(a, b);
        assert!(a.contains("10, 2"));
        for kind in [
            "DECIMAL",
            "DECIMALV2",
            "DECIMAL32",
            "DECIMAL64",
            "DECIMAL128",
            "NUMERIC",
            "NUMBER",
            "VARCHAR",
            "CHAR",
            "VARBINARY",
        ] {
            assert_ne!(
                template(&format!("SELECT CAST(id AS {kind}(10)) FROM t")).unwrap(),
                template(&format!("SELECT CAST(id AS {kind}(20)) FROM t")).unwrap(),
                "{kind}"
            );
        }
        assert!(template("SELECT CAST(id AS DECIMAL('private',2)) FROM t").is_err());
    }

    fn template(text: &str) -> Result<String, Unavailable> {
        normalize(text, Some("default_catalog"), Some("fp_oracle"))
    }

    #[test]
    fn lexer_follows_the_fixed_version_number_and_identifier_rules() {
        assert_eq!(
            template("SELECT 1e3, 1.5, .5, 7").unwrap(),
            "SELECT ?, ?, ?, ?"
        );
        // DIGIT_IDENTIFIER wins only when it is strictly longer.
        assert_eq!(
            template("SELECT 1e3x, 1_000 FROM t").unwrap(),
            "SELECT `1e3x`, `1_000` FROM t"
        );
        assert_eq!(
            template("SELECT t.5abc FROM t").unwrap(),
            "SELECT t.`5abc` FROM t"
        );
        assert_eq!(
            template("SELECT X'0aff', 'a\\'b', 'it''s', \"q\"\"q\"").unwrap(),
            "SELECT ?, ?, ?, ?"
        );
        assert_eq!(
            template("SELECT `a``b` FROM `select`").unwrap(),
            "SELECT `a``b` FROM `select`"
        );
    }

    #[test]
    fn golden_templates_follow_the_official_digest_classes() {
        for (text, expected) in [
            (
                "select  id\n FROM t where id=-1 /* c */",
                "SELECT id FROM t WHERE id = ?",
            ),
            (
                "SELECT id FROM t WHERE id IN (1, -2, '3') AND d = DATE '2024-01-01'",
                "SELECT id FROM t WHERE id IN (?) AND d = ?",
            ),
            (
                "SELECT id FROM t WHERE id NOT IN (1, amount)",
                "SELECT id FROM t WHERE id NOT IN (?, amount)",
            ),
            (
                "SELECT id FROM t WHERE name = NULL OR name IS NOT NULL OR flag = true",
                "SELECT id FROM t WHERE name = ? OR name IS NOT NULL OR flag = ?",
            ),
            (
                "SELECT id FROM t WHERE id <> 1 ORDER BY id ASC LIMIT 0, 10",
                "SELECT id FROM t WHERE id != ? ORDER BY id LIMIT ?",
            ),
            (
                "SELECT id FROM t LIMIT 10 OFFSET 5",
                "SELECT id FROM t LIMIT ?, ?",
            ),
            (
                "SELECT id FROM t ORDER BY 2 DESC, 1",
                "SELECT id FROM t ORDER BY 2 DESC, 1",
            ),
            (
                "SELECT case when id = 1 then 'a' end FROM t",
                "SELECT CASE WHEN id = ? THEN ? END FROM t",
            ),
            (
                "SELECT date_sub(d, interval 2 day) FROM t",
                "SELECT date_sub(d, INTERVAL ? day) FROM t",
            ),
            (
                "SELECT /*+ SET_VAR(query_timeout = 9) */ t.id FROM t INNER JOIN t2 ON t.id = t2.id",
                "SELECT t.id FROM t JOIN t2 ON t.id = t2.id",
            ),
            (
                "INSERT INTO t2 WITH LABEL l_1 VALUES (1, 'a'), (2, 'b')",
                "INSERT INTO t2 WITH LABEL ? VALUES (?, ?)",
            ),
        ] {
            assert_eq!(template(text).unwrap(), expected, "{text}");
        }
        assert_eq!(
            template("INSERT /*+ SET_VAR(a = 1) */ INTO t2 SELECT * FROM t"),
            Err(Unavailable::UnsupportedStatement)
        );
    }

    #[test]
    fn counterexamples_stay_apart() {
        for (left, right) in [
            // Column and alias spelling is printed as written.
            ("SELECT id FROM t", "SELECT ID FROM t"),
            ("SELECT x.id FROM t x", "SELECT y.id FROM t y"),
            ("SELECT * FROM t", "SELECT id, name FROM t"),
            // Structure, operators and predicate order.
            (
                "SELECT id FROM t WHERE id = 1 AND name = 'a'",
                "SELECT id FROM t WHERE name = 'a' AND id = 1",
            ),
            (
                "SELECT id FROM t WHERE id = 1",
                "SELECT id FROM t WHERE id = 1 AND id = 1",
            ),
            (
                "SELECT id FROM t WHERE id IN (1)",
                "SELECT id FROM t WHERE id = 1",
            ),
            (
                "SELECT id FROM t WHERE id IN (1)",
                "SELECT id FROM t WHERE id NOT IN (1)",
            ),
            (
                "SELECT id FROM t WHERE name IS NULL",
                "SELECT id FROM t WHERE name = NULL",
            ),
            ("SELECT id - 1 FROM t", "SELECT id, -1 FROM t"),
            ("SELECT id FROM t LIMIT 5, 10", "SELECT id FROM t LIMIT 10"),
            (
                "SELECT id FROM t ORDER BY id",
                "SELECT id FROM t ORDER BY id DESC",
            ),
            ("SELECT id FROM t ORDER BY 1", "SELECT id FROM t ORDER BY 2"),
            (
                "SELECT id FROM t UNION ALL SELECT id FROM t2",
                "SELECT id FROM t UNION SELECT id FROM t2",
            ),
            ("SELECT count(*) FROM t", "SELECT count(1) FROM t"),
            (
                "SELECT CAST(id AS VARCHAR) FROM t",
                "SELECT CAST(id AS BIGINT) FROM t",
            ),
            (
                "SELECT date_sub(d, INTERVAL 1 DAY) FROM t",
                "SELECT date_sub(d, INTERVAL 1 MONTH) FROM t",
            ),
            (
                "INSERT INTO t2 VALUES (1)",
                "INSERT INTO t2 (id) VALUES (1)",
            ),
            // Table names are case-sensitive; a quoted keyword is a name.
            ("SELECT id FROM T2", "SELECT id FROM t2"),
            ("SELECT `select` FROM t", "SELECT select FROM t"),
        ] {
            assert_ne!(
                template(left).ok(),
                template(right).ok(),
                "{left} | {right}"
            );
        }
    }

    #[test]
    fn unsupported_or_unreliable_text_is_unavailable() {
        for (text, reason) in [
            ("", Unavailable::EmptyStatement),
            ("  -- only a comment", Unavailable::EmptyStatement),
            ("SELECT 'open", Unavailable::LexicalError),
            ("SELECT 1 /* open", Unavailable::LexicalError),
            ("SELECT `open", Unavailable::LexicalError),
            (
                "SELECT 1 # hash is not a comment",
                Unavailable::LexicalError,
            ),
            ("SELECT $$x$$", Unavailable::LexicalError),
            ("SELECT (1", Unavailable::UnbalancedParentheses),
            ("SELECT 1)", Unavailable::UnbalancedParentheses),
            ("SELECT 1; SELECT 2", Unavailable::MultipleStatements),
            ("SHOW TABLES", Unavailable::UnsupportedStatement),
            ("EXPLAIN SELECT 1", Unavailable::UnsupportedStatement),
        ] {
            assert_eq!(template(text), Err(reason), "{text}");
        }
        assert!(template("SELECT 1;;").is_ok());
    }

    #[test]
    fn sql_null_and_truncated_statements_never_get_a_fingerprint() {
        let input = |statement, truncated| SqlFingerprintInput {
            statement,
            truncated,
            catalog: None,
            database: None,
        };
        assert_eq!(
            product_sql_fingerprint(input(None, false)),
            ProductSqlFingerprint::Unavailable {
                reason: Unavailable::StatementSqlNull
            }
        );
        assert_eq!(
            product_sql_fingerprint(input(Some("SELECT 1"), true)),
            ProductSqlFingerprint::Partial {
                reason: ProductSqlFingerprintPartialReason::StatementTruncated
            }
        );
        let ProductSqlFingerprint::Exact {
            fingerprint,
            template,
        } = product_sql_fingerprint(input(Some("select 1"), false))
        else {
            panic!("exact");
        };
        assert_eq!(template, "SELECT ?");
        assert_eq!(fingerprint.len(), 64);
    }

    #[test]
    fn qualifiers_fold_only_for_the_session_catalog_and_database() {
        assert_eq!(
            template("SELECT id FROM other.t").unwrap(),
            "SELECT id FROM other.t"
        );
        assert_eq!(
            template("SELECT id FROM fp_oracle.t").unwrap(),
            "SELECT id FROM t"
        );
        assert_eq!(
            template("SELECT id FROM default_catalog.fp_oracle.t").unwrap(),
            "SELECT id FROM t"
        );
        assert_eq!(
            template("SELECT id FROM hive.fp_oracle.t").unwrap(),
            "SELECT id FROM hive.fp_oracle.t"
        );
        // A column qualifier is printed as written by the official digest.
        assert_eq!(
            template("SELECT fp_oracle.t.id FROM t").unwrap(),
            "SELECT fp_oracle.t.id FROM t"
        );
        assert_eq!(
            normalize("SELECT id FROM fp_oracle.t", None, None).unwrap(),
            "SELECT id FROM fp_oracle.t"
        );
    }

    #[test]
    fn function_names_fold_but_table_names_before_parentheses_do_not() {
        assert_eq!(
            template("SELECT COUNT(*), Upper(name) FROM t").unwrap(),
            "SELECT count(*), upper(name) FROM t"
        );
        assert_eq!(
            template("INSERT INTO T2 (id) VALUES (1)").unwrap(),
            "INSERT INTO T2 (id) VALUES (?)"
        );
        assert_eq!(
            template("INSERT OVERWRITE T2 (id) SELECT 1").unwrap(),
            "INSERT OVERWRITE T2 (id) SELECT ?"
        );
        assert_eq!(
            template("WITH A (x) AS (SELECT 1) SELECT x FROM A").unwrap(),
            "WITH A (x) AS (SELECT ?) SELECT x FROM A"
        );
    }
}
