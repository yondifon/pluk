/**
 * Syntax highlighting — port of swift/Sources/SyntaxHighlight.swift
 *
 * Single-pass O(n) scanner for plain text, JSON, TOML, SQL, shell.
 * Console output uses line-level tinting only.
 * Palette deliberately distinct from verdict colours (green/orange/red).
 */

export type CodeLanguage = "text" | "json" | "toml" | "sql" | "shell";

export function parseLanguage(hint: string): CodeLanguage {
  const h = hint.trim().toLowerCase();
  if (h === "json") return "json";
  if (h === "toml") return "toml";
  if (h === "sql") return "sql";
  if (h === "shell" || h === "bash" || h === "sh") return "shell";
  return "text";
}

export type SyntaxTint = "comment" | "string" | "number" | "keyword" | "type" | "property";

export interface SyntaxSpan {
  start: number;
  end: number;
  tint: SyntaxTint;
}

// Light/dark RGB as in Swift
const lightPalette: Record<SyntaxTint, string> = {
  comment: "rgb(115,125,135)", // 0.45,0.49,0.53
  string: "rgb(13,110,102)",   // 0.05,0.43,0.40
  number: "rgb(110,79,176)",   // 0.43,0.31,0.69
  keyword: "rgb(153,51,92)",   // 0.60,0.20,0.36
  type: "rgb(71,92,181)",      // 0.28,0.36,0.71
  property: "rgb(33,97,166)",  // 0.13,0.38,0.65
};

const darkPalette: Record<SyntaxTint, string> = {
  comment: "rgb(158,166,176)",
  string: "rgb(115,194,184)",
  number: "rgb(181,158,240)",
  keyword: "rgb(222,153,184)",
  type: "rgb(166,174,245)",
  property: "rgb(133,176,235)",
};

export function tintColor(tint: SyntaxTint, isDark: boolean): string {
  return isDark ? darkPalette[tint] : lightPalette[tint];
}

function isDarkMode(): boolean {
  return typeof window !== "undefined" && window.matchMedia?.("(prefers-color-scheme: dark)").matches;
}

// ---- Dialect definitions ----
interface Dialect {
  lineComments: string[];
  blockComments: boolean;
  quotes: Set<string>;
  backslashEscapes: boolean;
  doubledQuotes: boolean;
  tripleQuotes: boolean;
  wordExtras: Set<string>;
  keywords: Set<string>;
  types: Set<string>;
  caseInsensitive: boolean;
  shell: boolean;
  propertyPrefix: string | null;
  headers: boolean;
}

const sqlKeywords = new Set([
  "select","from","where","insert","into","values","update","set","delete","create","drop","alter","table","view","index","join","inner","left","right","outer","full","cross","natural","on","using","and","or","not","is","null","distinct","group","order","by","having","limit","offset","union","all","intersect","except","exists","between","like","ilike","in","case","when","then","else","end","with","recursive","returning","primary","foreign","key","references","constraint","default","unique","check","add","column","rename","if","true","false","begin","commit","rollback","transaction","explain","analyze","as","asc","desc","nulls","first","last","window","partition","over","cast","collate",
]);

const sqlTypes = new Set([
  "integer","int","bigint","smallint","tinyint","serial","bigserial","numeric","decimal","real","double","float","money","text","varchar","char","character","boolean","bool","timestamp","timestamptz","datetime","date","time","timetz","interval","uuid","json","jsonb","bytea","blob","binary","varbinary","bit",
]);

const shellCommands = new Set([
  "cd","echo","head","cat","chmod","cp","curl","docker","git","grep","kill","ls","make","mv","open","pwd","rm","sed","ssh","sudo","tail","tar","touch","uname","whoami",
]);

const dialects: Record<CodeLanguage, Dialect | null> = {
  text: null,
  json: {
    lineComments: [], blockComments: false, quotes: new Set(['"']),
    backslashEscapes: true, doubledQuotes: false, tripleQuotes: false,
    wordExtras: new Set(), keywords: new Set(["true","false","null"]), types: new Set(),
    caseInsensitive: false, shell: false, propertyPrefix: ":", headers: false,
  },
  toml: {
    lineComments: ["#"], blockComments: false, quotes: new Set(['"',"'" ]),
    backslashEscapes: true, doubledQuotes: false, tripleQuotes: true,
    wordExtras: new Set(["-","."]), keywords: new Set(["true","false"]), types: new Set(),
    caseInsensitive: false, shell: false, propertyPrefix: "=", headers: true,
  },
  sql: {
    lineComments: ["--"], blockComments: true, quotes: new Set(["'","\""]),
    backslashEscapes: false, doubledQuotes: true, tripleQuotes: false,
    wordExtras: new Set(), keywords: sqlKeywords, types: sqlTypes,
    caseInsensitive: true, shell: false, propertyPrefix: null, headers: false,
  },
  shell: {
    lineComments: ["#"], blockComments: false, quotes: new Set(["'","\""]),
    backslashEscapes: true, doubledQuotes: false, tripleQuotes: false,
    wordExtras: new Set(["-",".","/","=",":","@"]), keywords: shellCommands, types: new Set(),
    caseInsensitive: false, shell: true, propertyPrefix: null, headers: false,
  },
};

function isWhitespace(ch: string): boolean {
  return ch === " " || ch === "\t" || ch === "\n" || ch === "\r";
}
function isDigit(ch: string): boolean { return ch >= "0" && ch <= "9"; }
function isLetter(ch: string): boolean { return (ch >= "a" && ch <= "z") || (ch >= "A" && ch <= "Z"); }
function isWordChar(ch: string, extras: Set<string>): boolean {
  return isLetter(ch) || isDigit(ch) || ch === "_" || extras.has(ch);
}

export function scan(source: string, language: CodeLanguage): SyntaxSpan[] {
  const dialect = dialects[language];
  if (!dialect) return [];
  const spans: SyntaxSpan[] = [];
  let i = 0;
  const n = source.length;

  const append = (start: number, end: number, tint: SyntaxTint) => {
    spans.push({ start, end, tint });
  };

  while (i < n) {
    const ch = source[i];
    if (isWhitespace(ch)) { i++; continue; }

    // Line comments
    let matchedComment: string | null = null;
    for (const pref of dialect.lineComments) {
      if (source.startsWith(pref, i)) { matchedComment = pref; break; }
    }
    if (matchedComment) {
      const start = i;
      while (i < n && source[i] !== "\n") i++;
      append(start, i, "comment");
      continue;
    }

    // Block comments (sql)
    if (dialect.blockComments && ch === "/" && i + 1 < n && source[i+1] === "*") {
      const start = i;
      i += 2;
      while (i < n) {
        if (source[i] === "*" && i + 1 < n && source[i+1] === "/") { i += 2; break; }
        i++;
      }
      append(start, i, "comment");
      continue;
    }

    // Strings
    if (dialect.quotes.has(ch)) {
      const start = i;
      // triple quotes
      if (dialect.tripleQuotes && source.startsWith(ch.repeat(3), i)) {
        const delim = ch.repeat(3);
        let j = i + 3;
        while (j < n && !source.startsWith(delim, j)) j++;
        if (j < n) j += 3;
        append(start, j, "string");
        i = j;
        continue;
      }
      let j = i + 1;
      while (j < n) {
        const c = source[j];
        if (c === ch) {
          if (dialect.doubledQuotes) {
            if (j + 1 < n && source[j+1] === ch) { j += 2; continue; }
            else { j++; break; }
          } else {
            j++; break;
          }
        } else if (dialect.backslashEscapes && c === "\\") {
          j += 2;
        } else {
          j++;
        }
      }
      // property key detection
      if (dialect.propertyPrefix) {
        let k = j;
        while (k < n && isWhitespace(source[k])) k++;
        if (k < n && source[k] === dialect.propertyPrefix) {
          append(start, j, "property");
          i = j;
          continue;
        }
      }
      append(start, j, "string");
      i = j;
      continue;
    }

    // Numbers with sign
    const next = i + 1 < n ? source[i+1] : "";
    if (isDigit(ch) || ((ch === "-" || ch === "+") && isDigit(next))) {
      const start = i;
      if (ch === "-" || ch === "+") i++;
      while (i < n && isDigit(source[i])) i++;
      if (i < n && source[i] === ".") {
        const after = i + 1 < n ? source[i+1] : "";
        if (isDigit(after)) {
          i += 2;
          while (i < n && isDigit(source[i])) i++;
        }
      }
      if (i < n && (source[i] === "e" || source[i] === "E")) {
        const after = i + 1 < n ? source[i+1] : "";
        const after2 = i + 2 < n ? source[i+2] : "";
        if (isDigit(after) || ((after === "-" || after === "+") && isDigit(after2))) {
          i++;
          if (source[i] === "-" || source[i] === "+") i++;
          while (i < n && isDigit(source[i])) i++;
        }
      }
      append(start, i, "number");
      continue;
    }

    // Words
    if (isLetter(ch) || ch === "_") {
      const start = i;
      while (i < n && isWordChar(source[i], dialect.wordExtras)) i++;
      const word = source.slice(start, i);
      const key = dialect.caseInsensitive ? word.toLowerCase() : word;
      if (dialect.keywords.has(key)) {
        append(start, i, dialect.shell ? "property" : "keyword");
      } else if (dialect.types.has(key)) {
        append(start, i, "type");
      } else if (dialect.propertyPrefix) {
        let k = i;
        while (k < n && isWhitespace(source[k])) k++;
        if (k < n && source[k] === dialect.propertyPrefix) append(start, i, "property");
      } else if (dialect.headers) {
        let k = i;
        while (k < n && isWhitespace(source[k])) k++;
        if (k < n && source[k] === "]") append(start, i, "property");
      }
      continue;
    }

    i++;
  }
  return spans;
}

// Console scanner — line-level only
const severityWords = new Set([
  "denied","err","error","errors","exception","fail","failed","failure","fatal","panic","refused","timeout","unauthorized","unhealthy","warn","warning",
]);
const banners = ["===","---","###"];

function isWordCharConsole(c: string): boolean {
  return isLetter(c) || isDigit(c) || c === "-" || c === ":" || c === "." || c === "_" || c === "+";
}
function isTimestampWord(w: string): boolean {
  if (!w[0] || !isDigit(w[0])) return false;
  return w.includes(":") || w.includes("-");
}
function bareWord(w: string): string {
  let s = 0; while (s < w.length && !isLetter(w[s])) s++;
  let e = s; while (e < w.length && isLetter(w[e])) e++;
  return w.slice(s, e).toLowerCase();
}

export function scanConsole(source: string): SyntaxSpan[] {
  const spans: SyntaxSpan[] = [];
  let lineStart = 0;
  for (let idx = 0; idx <= source.length; idx++) {
    const isEnd = idx === source.length || source[idx] === "\n";
    if (!isEnd) continue;
    const lineEnd = idx;
    // trim leading spaces
    let i = lineStart;
    while (i < lineEnd && (source[i] === " " || source[i] === "\t")) i++;
    if (i >= lineEnd) { lineStart = idx + 1; continue; }
    // banner check
    let isBanner = false;
    for (const b of banners) if (source.startsWith(b, i)) { isBanner = true; break; }
    if (isBanner) {
      spans.push({ start: i, end: lineEnd, tint: "property" });
      lineStart = idx + 1;
      continue;
    }
    let wordIndex = 0;
    let p = i;
    while (p < lineEnd) {
      if (!isWordCharConsole(source[p])) { p++; continue; }
      const s = p;
      while (p < lineEnd && isWordCharConsole(source[p])) p++;
      const word = source.slice(s, p);
      if (wordIndex < 2 && isTimestampWord(word)) {
        spans.push({ start: s, end: p, tint: "comment" });
      } else if (severityWords.has(bareWord(word))) {
        spans.push({ start: s, end: p, tint: "keyword" });
      }
      wordIndex++;
    }
    lineStart = idx + 1;
  }
  return spans;
}

export function escapeHtml(s: string): string {
  return s.replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;").replace(/"/g,"&quot;");
}

export function highlightedHtml(source: string, language: CodeLanguage): string {
  const spans = scan(source, language);
  return spansToHtml(source, spans);
}

export function consoleHtml(source: string): string {
  const spans = scanConsole(source);
  return spansToHtml(source, spans);
}

function spansToHtml(source: string, spans: SyntaxSpan[]): string {
  const dark = isDarkMode();
  if (spans.length === 0) return escapeHtml(source);
  // spans are already in order
  let out = "";
  let cursor = 0;
  for (const sp of spans) {
    if (sp.start > cursor) out += escapeHtml(source.slice(cursor, sp.start));
    const col = tintColor(sp.tint, dark);
    out += `<span style="color:${col}">${escapeHtml(source.slice(sp.start, sp.end))}</span>`;
    cursor = sp.end;
  }
  if (cursor < source.length) out += escapeHtml(source.slice(cursor));
  return out;
}

/** Async off-main-thread highlighting for large payloads */
export function highlightedHtmlAsync(source: string, language: CodeLanguage): Promise<string> {
  // Use setTimeout to yield to browser; real worker not needed for this size
  return new Promise(resolve => {
    if (source.length < 5000) { resolve(highlightedHtml(source, language)); return; }
    setTimeout(() => resolve(highlightedHtml(source, language)), 0);
  });
}
