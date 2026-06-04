const SECRET_PATTERNS = [
  /\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/gi,
  /\b(sk-[A-Za-z0-9_-]{16,})\b/g,
  /\b(xox[baprs]-[A-Za-z0-9-]{16,})\b/g,
  /\b(gh[pousr]_[A-Za-z0-9_]{16,})\b/g,
  /\b([A-Za-z0-9._%+-]+:[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,})\b/g,
  /\b((?:api[_-]?key|token|secret|password|authorization|bearer)\s*[:=]\s*)("[^"]+"|'[^']+'|[^\s,;]+)/gi,
];

function redactText(value) {
  if (value == null) return "";
  let text = String(value);
  for (const pattern of SECRET_PATTERNS) {
    text = text.replace(pattern, (match, prefix) => {
      if (prefix && /\s*[:=]\s*$/i.test(prefix)) {
        return `${prefix}[redacted]`;
      }
      return "[redacted]";
    });
  }
  return text;
}

function truncate(value, maxLength) {
  const text = String(value || "");
  if (text.length <= maxLength) return text;
  return `${text.slice(0, Math.max(0, maxLength - 24)).trimEnd()}\n...[truncated]`;
}

function cleanText(value, maxLength = 4000) {
  return truncate(redactText(value).trim(), maxLength);
}

module.exports = {
  cleanText,
  redactText,
  truncate,
};
