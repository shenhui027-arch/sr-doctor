/**
 * G2 calibration: SQL literal masking for locally exported production samples.
 *
 * Masking keeps the statement's shape (keywords, identifiers, operators, IN-list
 * lengths) so fingerprints computed on the masked text can still be compared
 * with the official digest, while every literal value is replaced:
 * strings -> 'S', numbers -> 0, comment bodies dropped. Identifiers are kept
 * unless the caller asks for consistent keyed pseudonyms.
 *
 * This is a scanner, not a parser: an unterminated quote or comment (usually a
 * truncated audit stmt) masks everything after it and is reported.
 */
import { createHmac } from "node:crypto";

export const MASK_VERSION = "g2-sql-mask-v1";
/** This tool's own statement marker; kept so tool rows stay identifiable. */
export const DOCTOR_MARKER = "/* sr-doctor */";
/** The marker earlier releases used; audit history still carries it. */
export const LEGACY_DOCTOR_MARKER = "/* starrocks-doctor */";
const DOCTOR_MARKERS = [DOCTOR_MARKER, LEGACY_DOCTOR_MARKER];

// Words never pseudonymized: SQL keywords, type names and interval units.
// Function names are recognised by a following "(" instead of being listed.
const KEYWORDS = new Set(`
add all alter analyze and anti any array as asc between bigint binary bitmap boolean both by case cast char
collate column columns convert create cross current date datetime day decimal default delete desc describe
distinct div double drop else end escape except exists explain false first float following for from full
group having hll hour if ignore in inner insert int integer intersect interval into is join json key largeint
last lateral leading left like limit map microsecond minute mod month natural not null nulls offset on or
order outer over partition preceding quarter range regexp right rlike row rows second select semi set show
smallint some straight_join string struct table then tinyint to trailing true unbounded union update using
values varchar week when where window with xor year
`.trim().split(/\s+/));

const IDENT_START = /[A-Za-z_$\u0080-￿]/;
const IDENT_PART = /[A-Za-z0-9_$\u0080-￿]/;
const NUMBER = /^(?:0x[0-9a-f]+|\d+\.?\d*(?:e[+-]?\d+)?|\.\d+(?:e[+-]?\d+)?)/i;

/** Splits SQL into tokens. Never throws; unterminated constructs end the scan. */
export function tokenizeSql(sql) {
  const tokens = [];
  let i = 0;
  const significant = () => {
    for (let k = tokens.length - 1; k >= 0; k -= 1) if (tokens[k].type !== "ws" && tokens[k].type !== "comment") return tokens[k];
    return null;
  };
  while (i < sql.length) {
    const c = sql[i];
    const next = sql[i + 1];
    if (/\s/.test(c)) {
      let j = i + 1;
      while (j < sql.length && /\s/.test(sql[j])) j += 1;
      tokens.push({ type: "ws", text: sql.slice(i, j) });
      i = j;
    } else if ((c === "-" && next === "-" && (i + 2 >= sql.length || /\s/.test(sql[i + 2]))) || c === "#") {
      let j = sql.indexOf("\n", i);
      if (j < 0) j = sql.length;
      tokens.push({ type: "comment", text: sql.slice(i, j), line: true });
      i = j;
    } else if (c === "/" && next === "*") {
      const end = sql.indexOf("*/", i + 2);
      if (end < 0) {
        tokens.push({ type: "comment", text: sql.slice(i), unterminated: true });
        i = sql.length;
      } else {
        tokens.push({ type: "comment", text: sql.slice(i, end + 2) });
        i = end + 2;
      }
    } else if (c === "'" || c === '"' || c === "`") {
      let j = i + 1;
      let closed = false;
      while (j < sql.length) {
        if (c !== "`" && sql[j] === "\\") { j += 2; continue; }
        if (sql[j] === c) {
          if (sql[j + 1] === c) { j += 2; continue; }
          closed = true;
          j += 1;
          break;
        }
        j += 1;
      }
      const text = sql.slice(i, Math.min(j, sql.length));
      tokens.push({ type: c === "`" ? "quoted_ident" : "string", quote: c, text, unterminated: !closed });
      i = closed ? j : sql.length;
    } else if (/\d/.test(c) || (c === "." && /\d/.test(next ?? "") && !["ident", "quoted_ident", "number"].includes(significant()?.type) && significant()?.text !== ")")) {
      const match = sql.slice(i).match(NUMBER);
      const end = i + (match ? match[0].length : 1);
      if (end < sql.length && IDENT_PART.test(sql[end]) && sql[end] !== ".") {
        // Digits glued to letters (1abc) form an identifier, not a literal.
        let j = end;
        while (j < sql.length && IDENT_PART.test(sql[j])) j += 1;
        tokens.push({ type: "ident", text: sql.slice(i, j) });
        i = j;
      } else {
        tokens.push({ type: "number", text: sql.slice(i, end) });
        i = end;
      }
    } else if (IDENT_START.test(c)) {
      let j = i + 1;
      while (j < sql.length && IDENT_PART.test(sql[j])) j += 1;
      tokens.push({ type: "ident", text: sql.slice(i, j) });
      i = j;
    } else {
      tokens.push({ type: "op", text: c });
      i += 1;
    }
  }
  return tokens;
}

/** Keyed, stable pseudonym for an identifier or identity value. */
export function pseudonym(key, prefix, value) {
  return `${prefix}_${createHmac("sha256", key).update(String(value).toLowerCase()).digest("hex").slice(0, 8)}`;
}

function nextSignificant(tokens, index) {
  for (let k = index + 1; k < tokens.length; k += 1) if (tokens[k].type !== "ws") return tokens[k];
  return null;
}

/**
 * Masks literals in one SQL text.
 * options.key + options.pseudonymizeIdentifiers replaces identifiers (not
 * keywords, not function names) with stable `i_xxxxxxxx` pseudonyms.
 */
export function maskSql(sql, options = {}) {
  const tokens = tokenizeSql(sql);
  const stats = { strings: 0, numbers: 0, comments: 0, identifiers: 0, unterminated: false };
  const out = tokens.map((token, index) => {
    switch (token.type) {
      case "string":
        stats.strings += 1;
        if (token.unterminated) stats.unterminated = true;
        return `${token.quote}S${token.quote}`;
      case "number":
        stats.numbers += 1;
        return "0";
      case "comment":
        if (token.unterminated) { stats.unterminated = true; stats.comments += 1; return "/* */"; }
        if (DOCTOR_MARKERS.includes(token.text)) return token.text;
        stats.comments += 1;
        return token.line ? "--" : "/* */";
      case "quoted_ident":
        if (token.unterminated) { stats.unterminated = true; return "`S`"; }
        if (!options.pseudonymizeIdentifiers) return token.text;
        stats.identifiers += 1;
        return `\`${pseudonym(options.key, "i", token.text.slice(1, -1).replaceAll("``", "`"))}\``;
      case "ident": {
        if (!options.pseudonymizeIdentifiers || KEYWORDS.has(token.text.toLowerCase())) return token.text;
        if (nextSignificant(tokens, index)?.text === "(") return token.text;
        stats.identifiers += 1;
        return pseudonym(options.key, "i", token.text);
      }
      default:
        return token.text;
    }
  });
  return { text: out.join(""), stats };
}

/** True when masked SQL still contains a literal value: used by the verifier. */
export function residualLiterals(sql) {
  const found = [];
  for (const token of tokenizeSql(sql)) {
    if (token.type === "string" && token.text !== `${token.quote}S${token.quote}`) found.push("string_literal");
    else if (token.type === "number" && token.text !== "0") found.push("numeric_literal");
    else if (token.type === "comment" && !["--", "/* */", ...DOCTOR_MARKERS].includes(token.text.trim())) found.push("comment_content");
  }
  return found;
}
